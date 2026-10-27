// prd-zen-user-widgets-v2 UW15–UW19, UW26, UW29, UWA4, UWA6, UWA10,
// UWB8–UWB11 — the custom layer: what a sealed widget's frame may ask K2
// for, checked on every message before anything reaches K2's own verbs.
// No permissions (Rosson 2026-10-08): a widget in your own Garden may use
// every Garden-safe verb it asks for, on the Garden's reach, at once.
//
// One layer per placement (one private port, UW15): who is calling comes
// from which port the message arrived on, never from what it says. Each
// request goes through, in order:
//   1. size: a message over 64 KB → `too_large`;
//   2. budget: 60 calls a second (burst 120) → `rate_limited`; ten
//      `rate_limited` answers in a minute unload the widget (UW29);
//   3. the allowlist (`ZEN_CUSTOM_VERBS`, generated from the catalog, never
//      `ZEN_VERBS`): a catalog verb not marked `widget` → `not_exposed`
//      (`zen.exit`, `controls.bind`, `homes.list`, …); anything else →
//      `unknown_verb` (UWA10);
//   4. the cap: the manifest's caps ∩ the four widget caps, as the daemon
//      sent them and the renderer intersected again (UW26) →
//      `cap_not_granted` (a verb the widget didn't ask for);
//   5. the reach: an address must be one of the widget's rows (this
//      computer's agents plus your Homes' rows now, `zenWidgetReachRows`)
//      → `not_bound`. Thread verbs take any of them (UWB10);
//   6. posting: text ≤ 4,000 characters, never paths, uploads or secrets
//      (UW19, UW50); files ride only as `attachments` the person picked
//      with K2's own picker (`thread.pickFiles`, 0.46.3: ≤ 10 workspace-
//      relative paths, which the agent's daemon checks again); every post and card answer carries the widget's
//      Garden, placement and frame hash, and the daemon holding the Garden
//      refuses it (`widget_not_approved` / `widget_cap_missing`) unless
//      that placement's approved caps include `thread:post` (0.46.1); the
//      runaway guard (more than 120 posts, or 20
//      identical texts to one agent, in 10 minutes; R6) pauses posting
//      (`sending_off`) until the person clicks Resume on the widget. The
//      frame keeps running;
//   7. limits: 16 live subscriptions, 16 Thread subscriptions, at most 8
//      live servers (`zen-custom-scope`), `gardens.switch` once every 2 s
//      and never in the first 2 s after mount; `canvas.setHitRegions` at
//      most 32 finite rects (`zen-canvas`), `window.startDrag` only during
//      a press in that frame; `layout.*` (Garden columns, 0.46.3 r10) on
//      the caller's own page only, 4 a second per verb, a fold or unfold
//      only within 5 s of a press in that frame, saved per Garden on this
//      computer like the person's own click (0.46.4); `links.open`
//      (`k2.open`, 0.46.4) only within 5 s of a press in that frame, once a
//      second and 10 a minute, one card at a time (`zen-links`);
//      `downloads.*` (`k2.download`, 0.46.5) the same press rule, once
//      every 2 s and 10 a minute, one at a time; a `downloads.begin` ticket
//      lets the bytes follow within 60 s (`zen-custom-blobs`).
// 0.46.5 (prd-garden-board-needs-v1 GB12): bytes ride beside a message as
// one transferred ArrayBuffer (`bytes`), only on `blobs.put` and
// `downloads.save`, at most 16 / 50 MiB; the 64 KB check can't see them
// (an ArrayBuffer stringifies to `{}`), so step 1 checks them itself. A
// `blobs.get` reply moves its buffer back the same way.
// 0.46.3 Thread parity (Rosson 2026-10-20): `thread.read` takes `afterSeq`
// (a delta: items, hasMore, changed, removed), and `thread.markRead`,
// `thread.attachment`, `thread.open`, `thread.links` and `thread.pickFiles`
// are answered (`zen-thread-files`); attachments cross as the catalog's
// ThreadFile (never a path), a hidden one as `{kind, state}`. The picker
// opens only after the person's own press in the frame (or with its focus),
// one at a time.
// Then K2's own verb runs through the widget's inner bridge (built with the
// effective caps) and the answer is projected to the guest shape
// (`zen-custom-projection`) and cloned as plain JSON (functions dropped).
// Renderer-side checks are the second of three (UW26): the daemon decided
// which caps the placement may claim, and every server re-checks your role.

import { ZenBridgeError, ZEN_VERBS, type ZenWidgetBridge } from './zen-bridge'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'
import { USER_WIDGET_CAPS, type UserWidgetCap } from '../k2-caps.generated'
import type { CatalogErrorCode, CatalogWireError } from '../contract/catalog-types'
import { ZEN_AGENTS_SHOW_MAX, type ZenCustomWidgetPayload, type ZenFrameReply, type ZenFrameRequest } from './zen-custom-types'
import type { ZenAgentRow, ZenPerson, ZenThreadView } from './zen-data'
import { ZEN_LINKS_OPEN, zenLinkTarget } from './zen-links'
import {
  projectZenActivityState,
  zenEndedAtSubscribe,
  projectZenPeople,
  projectZenRows,
  projectZenThreadDelta,
  projectZenThreadItem,
  projectZenThreadItems,
  projectZenThreadView,
} from './zen-custom-projection'
import { ZenThreadVerbError, projectZenThreadFileData, zenThreadAttachments, type ZenThreadDelta } from './zen-thread-files'
import type { ZenPermissionSheet } from './permission-sheet'
import { DaemonHttpError } from '../daemon-http-error'
import type { ZenInboxMessage } from './zen-thread-inbox'
import type { ZenWidgetStopReason } from './zen-custom-run'
import { ZenCanvasError, zenHitRegionsFrom } from './zen-canvas'
import type { ZenRect } from './zen-controls'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { canonicalSourceAddress, ZenSourceError, type ZenSourcesReader } from './zen-sources'
import { ZEN_STORE_VERBS, ZenStoreError, type ZenCustomStore } from './zen-custom-store'
import { ZEN_GUEST_VERBS, ZenGuestsError, type ZenCustomGuests } from './zen-custom-guests'
import { ZenWidgetError } from './zen-widget-routes'
import type { ZenColumnFoldRefusal, ZenLayoutColumn } from './zen-columns'
import { ZEN_THREAD_CONTEXT_PER_SECOND, clearZenThreadContextFor, zenThreadContextProblem } from './zen-thread-context'
import {
  ZEN_BLOB_REF,
  ZEN_BYTE_VERBS,
  ZEN_DOWNLOADS,
  isArrayBuffer,
  zenBytesReader,
  zenDownloadName,
  type ZenCustomBlobs,
} from './zen-custom-blobs'
import { zenStoreErrorFrom } from './zen-custom-store'
import type { DownloadSliceReader } from '../fs-transfer'

/** UW29 budgets. */
export const ZEN_WIDGET_BUDGET = {
  callsPerSecond: 60,
  burst: 120,
  liveSubscriptions: 16,
  threadSubscriptions: 16,
  messageBytes: 64 * 1024,
  rateLimitedPerMinute: 10,
  switchEveryMs: 2_000,
  switchAfterMountMs: 2_000,
  postChars: 4_000,
} as const

/** How recent the person's press in a frame must be for `thread.pickFiles`. */
export const ZEN_PICK_GESTURE_MS = 5_000

/** R6: the runaway guard, per widget, over a sliding 10 minutes. */
export const ZEN_RUNAWAY = { windowMs: 10 * 60_000, posts: 120, identical: 20 } as const

/** What a refused post says while the guard has posting paused. */
export const ZEN_PAUSED_MESSAGE = 'Paused: too many posts. Resume it on the widget.'

export class ZenCustomRefusal extends Error {
  readonly wire: CatalogWireError
  constructor(
    code: CatalogErrorCode,
    message: string,
    extra: Partial<Pick<CatalogWireError, 'cap' | 'room' | 'feature' | 'last'>> = {},
  ) {
    super(message)
    this.name = 'ZenCustomRefusal'
    this.wire = { code, message, ...extra }
  }
}

/** What the layer needs from its frame host. */
export interface ZenCustomLayerDeps {
  /** The placement as the daemon resolved it (live: re-read each call). */
  widget(): ZenCustomWidgetPayload
  gardenId(): string
  /** The bundle hash this frame runs (`widget/bundle`), else the
   *  placement's `hash`. Every Thread post and card answer names it with
   *  the placement, so the daemon holding the Garden can check the
   *  widget's approved `thread:post` (0.46.1). */
  frameHash?(): string
  /** The widget's inner bridge, built with its caps. */
  inner: ZenWidgetBridge
  /** The rows it reaches now (never a fallback). */
  boundRows(): ZenAgentRow[]
  /** UWB10: resolve a bound agent's conversation without picking it. */
  ensureConversation(address: string): Promise<void>
  /** Push a subscription value to the frame. */
  push(sub: number, value: unknown): void
  /** End a subscription with a refusal after it started (an async failure,
   *  e.g. a source's server that isn't saved): the frame's onError hears it. */
  pushError?(sub: number, error: CatalogWireError): void
  /** Garden v3 data sources (contract-sources-v1 §6): the addresses this
   *  placement names (its config / manifest), the reader, and K2's Ask box.
   *  Without it the `sources.*` verbs answer `verb_unavailable`. */
  sources?: ZenCustomSources
  /** 0.46.1: timers for a `not_bound` `sources.subscribe`, held for
   *  `ZEN_SOURCES_REBIND_MS` so a placement change can bind it (see
   *  `boundChanged`). Without them a refusal is answered at once. */
  setTimer?(fn: () => void, ms: number): unknown
  clearTimer?(t: unknown): void
  /** Garden data (contract-garden-data-v1 §7): the widget's own data on
   *  this computer (`store.*`, `save.*`, `viewer.get`, and subscribe on
   *  `store.changes`). Without it those verbs answer `verb_unavailable`. */
  store?: ZenCustomStore
  /** RS-A15: local Apps' guests' rooms (`guests.rooms` / `guests.setRooms`)
   *  through this computer's daemon; every write goes through K2's confirm
   *  sheet outside the frame. Without it those verbs answer `verb_unavailable`. */
  guests?: ZenCustomGuests
  /** 0.46.3 permission cards (Beta): `thread.answer` on a permission card
   *  opens K2's sheet outside the frame instead of answering (a widget
   *  never answers one). Without it the daemon's own check refuses. */
  permissions?: ZenPermissionSheet
  /** Stop this widget (its frame goes; K2 draws the card). */
  stop(reason: ZenWidgetStopReason): void
  /** Did the person just press in this frame (or does it have the focus)?
   *  `thread.pickFiles` opens K2's picker only then. Absent: allowed. */
  gesture?(): boolean
  /** Is posting paused now (the runaway guard, here or in the daemon)? */
  paused(): boolean
  /** The runaway guard tripped: pause posting (this window at once, and
   *  the daemon for every window) until the person's Resume. */
  pauseForRunaway(): void
  /** The Garden's theme now (`ThemeInfo`), and its changes (UW39). */
  theme(): unknown
  onThemeChange(cb: () => void): () => void
  /** The whole canvas (`zen-canvas`): this frame's hit regions (checked
   *  frame-coordinate rects) and its window drag. Without it the two
   *  verbs answer `verb_unavailable`. */
  canvas?: {
    setHitRegions(rects: ZenRect[]): void
    /** Throws `ZenCanvasError` when the press checks fail. */
    startDrag(): void
  }
  /** prd-garden-column-collapse A1 (CA10): the columns of the page this
   *  frame is drawn on, and no other (`ZenColumnsContext`). Without it the
   *  `layout.*` verbs answer `verb_unavailable`. */
  layout?: ZenCustomLayout
  /** `k2.open(url)` (0.46.4): the person's browser, after K2's own card the
   *  first time per site in this Garden. Without it `links.open` answers
   *  `verb_unavailable`. */
  links?: ZenCustomLinks
  /** prd-garden-board-needs-v1 GB12: this widget's files on this computer
   *  (the daemon's widget door). Without it `blobs.put` / `blobs.get`
   *  answer `verb_unavailable`. */
  blobs?: ZenCustomBlobs
  /** GS1–GS7: K2's own download path. Without it `downloads.*` answer
   *  `verb_unavailable` (the widget preview, a headless daemon). */
  downloads?: ZenCustomDownloads
  now(): number
}

/** What the layer needs for `downloads.*` (`k2.download`). */
export interface ZenCustomDownloads {
  /** Did the person press in this frame within the last 5 s? (A press
   *  only; the frame having the focus is not enough.) */
  pressed(): boolean
  /** K2's download lifecycle (`downloadChunks`): the path or file name it
   *  landed as, or null when the person cancelled or it failed (K2 said so). */
  save(name: string, read: DownloadSliceReader): Promise<string | null>
}

/** A call's answer that carries bytes back to the frame (moved, not copied). */
export class ZenBytesValue {
  constructor(
    readonly value: unknown,
    readonly bytes: ArrayBuffer,
  ) {}
}

/** What the layer needs for `links.open` (`zen-links`). */
export interface ZenCustomLinks {
  /** Did the person press in this frame within the last 5 s? (A press
   *  only; the frame having the focus is not enough.) */
  pressed(): boolean
  /** Open an already checked http(s) link for this Garden: K2's card the
   *  first time per site; true when it opened. */
  open(url: string, site: string): Promise<boolean>
}

/** What the layer needs for `k2.layout.*` (frame-only verbs, no cap). */
export interface ZenCustomLayout {
  columns(): ZenLayoutColumn[]
  /** A widget's fold, saved like the person's click (the press is checked
   *  here first). Null when done or nothing changed. */
  setByWidget(index: number, collapsed: boolean): ZenColumnFoldRefusal | null
  subscribe(fn: (columns: ZenLayoutColumn[]) => void): () => void
  /** Did the person press in this frame within the last 5 s? (A press
   *  only; the frame having the focus is not enough.) */
  pressed(): boolean
}

/** CA10: each `layout.*` verb at most this many times a second. */
export const ZEN_LAYOUT_PER_SECOND = 4

/** What the custom layer needs for the `sources.*` verbs. */
export interface ZenCustomSources {
  /** The placement's bound addresses (`sourceAddresses` from the daemon). */
  bound(): readonly string[]
  reader: ZenSourcesReader
  /** Open K2's Ask an agent box about this source (K2 writes the draft). */
  ask(address: string, itemKey: string | null): Promise<boolean>
}

export interface ZenCustomLayer {
  /** One message from the frame's port; the reply to post, if any. */
  handle(msg: unknown): Promise<ZenFrameReply | null>
  /** 0.46.1: the placement's `sourceAddresses` changed (a config edit, an
   *  approval). Held `not_bound` source subscriptions whose address is now
   *  bound start under their own ids; the widget never saw a refusal. */
  boundChanged(): void
  /** Live subscriptions (tests). */
  readonly subscriptions: number
  /** Source subscriptions held after a `not_bound` (tests). */
  readonly held: number
  dispose(): void
}

/** What a retired `k2.sources.ask` answers (code `retired`, 0.46.4). */
export const ZEN_SOURCES_ASK_RETIRED =
  'k2.sources.ask is retired: K2 no longer opens an Ask an agent box from a card (0.46.4), and the verb goes in 0.47.0.'

/** How long a `not_bound` `sources.subscribe` waits for the placement to
 *  bind its address before the refusal reaches the widget (0.46.1 S3). A
 *  refused subscription ends in the frame (its onError), so K2 holds it
 *  instead and starts it if `boundChanged` brings the address in time. */
export const ZEN_SOURCES_REBIND_MS = 30_000

type Row = { cap: UserWidgetCap | null; also?: readonly UserWidgetCap[]; kind: 'call' | 'subscribe' | 'event' }

const CUSTOM: Record<string, Row> = ZEN_CUSTOM_VERBS as unknown as Record<string, Row>
const WIDGET_CAPS: ReadonlySet<string> = new Set(USER_WIDGET_CAPS)

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** The cap a card reads data sources with (Garden v3, contract §6). */
export const SOURCES_READ_CAP = 'sources:read'

/** `k2.store.subscribe` rides a subscription on this `garden-local` call
 *  row (contract-garden-data-v1 §7.2: subscribe is an SDK helper). */
export const ZEN_STORE_SUBSCRIBE_VERB = 'store.changes'

/**
 * The caps a placement runs with, intersected again here (UW26 step 2:
 * `source: user` whatever the body said). **Sources widgets keep what they
 * ask for (Rosson 2026-10-17, 0.46.1):** a widget that asks for
 * `sources:read` may also post to the Thread (`thread:post`) and write its
 * own store (`store:write`) when its manifest asks for those caps, the
 * same rule as the daemon's `effective_caps`. A cap it didn't ask for is
 * still refused (`cap_not_granted`), and the runaway guard still counts
 * every post.
 */
export function zenEffectiveCaps(w: Pick<ZenCustomWidgetPayload, 'caps' | 'requested'>): ReadonlySet<string> {
  const requested = w.requested as readonly string[]
  return new Set((w.caps as readonly string[]).filter((c) => WIDGET_CAPS.has(c) && requested.includes(c)))
}

/** Values cross as plain JSON: functions, prototypes and cycles never do. */
export function zenPlain(value: unknown): unknown {
  if (value === undefined) return null
  return JSON.parse(JSON.stringify(value)) as unknown
}

function handleOf(address: string): string {
  const i = address.indexOf('::')
  return i > 0 ? address.slice(0, i) : address
}

export function createZenCustomLayer(deps: ZenCustomLayerDeps): ZenCustomLayer {
  const mountedAt = deps.now()
  let tokens: number = ZEN_WIDGET_BUDGET.burst
  let lastRefill = mountedAt
  const limited: number[] = []
  const posts: Array<{ at: number; key: string }> = []
  let lastSwitch = -Infinity
  let stopped = false
  /** 0.46.3 r10: clears this widget's `agents.show` list (when it stops or unloads). */
  let unshow: (() => void) | null = null
  const subs = new Map<number, { off: () => void; thread: boolean; budget: boolean }>()
  /** 0.46.1: `not_bound` source subscriptions held for a rebind (their
   *  `subs` entry stays, so they count toward the 16). */
  const held = new Map<number, { start: () => void; address: string | null; wire: CatalogWireError; timer: unknown }>()

  const caps = (): ReadonlySet<string> => zenEffectiveCaps(deps.widget())

  const canHold = (verb: string, wire: CatalogWireError): boolean =>
    verb === 'sources.subscribe' && wire.code === 'not_bound' && !!deps.setTimer && !!deps.clearTimer

  /** Hold a refused source subscription; its refusal is answered (through
   *  `pushError`) only if no rebind starts it within ZEN_SOURCES_REBIND_MS. */
  const hold = (sub: number, address: string | null, wire: CatalogWireError, start: () => void): void => {
    const timer = deps.setTimer!(() => {
      const h = held.get(sub)
      if (!h || h.timer !== timer) return
      held.delete(sub)
      if (stopped || !subs.has(sub)) return
      subs.delete(sub)
      deps.pushError?.(sub, wire)
    }, ZEN_SOURCES_REBIND_MS)
    held.set(sub, { start, address, wire, timer })
  }

  const dropHeld = (sub: number): void => {
    const h = held.get(sub)
    if (!h) return
    held.delete(sub)
    deps.clearTimer?.(h.timer)
  }

  const stop = (reason: ZenWidgetStopReason): void => {
    if (stopped) return
    stopped = true
    dispose()
    deps.stop(reason)
  }

  const noteLimited = (): void => {
    const now = deps.now()
    limited.push(now)
    while (limited.length > 0 && now - limited[0] > 60_000) limited.shift()
    if (limited.length >= ZEN_WIDGET_BUDGET.rateLimitedPerMinute) stop('rate')
  }

  const refuse = (code: CatalogErrorCode, message: string, extra: Partial<CatalogWireError> = {}): never => {
    if (code === 'rate_limited') noteLimited()
    throw new ZenCustomRefusal(code, message, extra)
  }

  const takeToken = (): void => {
    const now = deps.now()
    tokens = Math.min(ZEN_WIDGET_BUDGET.burst, tokens + ((now - lastRefill) / 1000) * ZEN_WIDGET_BUDGET.callsPerSecond)
    lastRefill = now
    if (tokens < 1) refuse('rate_limited', 'Over 60 calls a second.')
    tokens -= 1
  }

  const needAddress = (verb: string, v: unknown): string => {
    if (typeof v !== 'string' || !v.trim()) refuse('failed', `${verb} needs an agent address`)
    const address = (v as string).trim().toLowerCase()
    if (!deps.boundRows().some((r) => r.address === address)) {
      refuse('not_bound', `${address} isn’t one of this widget’s agents.`, { room: handleOf(address) })
    }
    return address
  }

  /** Garden v3: an address this placement names (config / manifest), else `not_bound`. */
  const needSource = (verb: string, v: unknown): { address: string; src: ZenCustomSources } => {
    const src = deps.sources
    if (!src) refuse('verb_unavailable', `${verb} isn’t available here.`)
    if (typeof v !== 'string' || !v.trim()) refuse('failed', `${verb} needs a source address`)
    const address = canonicalSourceAddress(v as string)
    if (!address) refuse('failed', `'${String(v)}' isn’t a source address (name or name::host).`)
    const bound = new Set(src!.bound().map((a) => canonicalSourceAddress(a)).filter((a): a is string => a !== null))
    if (!bound.has(address!)) {
      refuse('not_bound', `${address} isn’t one of this placement’s sources (config.source / config.sources).`, { room: address! })
    }
    return { address: address!, src: src! }
  }

  /** A reader failure as the bridge's refusal (`unreachable` carries `last`). */
  const sourceRefusal = (address: string, err: unknown): ZenCustomRefusal => {
    if (!(err instanceof ZenSourceError)) return toRefusal(err)
    if (err.code === 'rate_limited') noteLimited()
    const extra: Partial<CatalogWireError> = { room: address }
    if (err.last) extra.last = err.last
    const msg = err.code === 'cap_not_granted' && err.role ? `${err.message} (needs ${err.role})` : err.message
    return new ZenCustomRefusal(err.code, msg, extra)
  }

  const sourceCall = async <T>(address: string, op: () => Promise<T>): Promise<T> => {
    try {
      return await op()
    } catch (err) {
      throw sourceRefusal(address, err)
    }
  }

  const needInt = (verb: string, what: string, v: unknown, min: number): number => {
    if (typeof v !== 'number' || !Number.isInteger(v) || v < min) refuse('bad_args', `${verb}: ${what} is a whole number ≥ ${min}`)
    return v as number
  }

  let picking = false
  /** `conversation.setContext` calls in the last second, and whether this
   *  widget holds a context now (cleared when it stops or unloads). */
  const contextCalls: number[] = []
  let contextSet = false

  const needText = (verb: string, v: unknown): string => {
    if (typeof v !== 'string') refuse('failed', `${verb} needs text`)
    const text = v as string
    if ([...text].length > ZEN_WIDGET_BUDGET.postChars) refuse('too_large', `${verb} takes up to 4,000 characters`)
    return text
  }

  let wasPaused = false
  /** Posting is refused while paused; after a Resume the count starts over. */
  const needSending = (): void => {
    if (deps.paused()) {
      wasPaused = true
      refuse('sending_off', ZEN_PAUSED_MESSAGE)
    }
    if (wasPaused) {
      wasPaused = false
      posts.length = 0
    }
  }

  /** R6: record a post; trip the guard on the 121st, or the 21st identical text to one agent. */
  const guard = (address: string, text: string): void => {
    const now = deps.now()
    while (posts.length > 0 && now - posts[0].at > ZEN_RUNAWAY.windowMs) posts.shift()
    const key = `${address}\n${text.trim()}`
    const same = posts.filter((p) => p.key === key).length
    if (posts.length + 1 > ZEN_RUNAWAY.posts || same + 1 > ZEN_RUNAWAY.identical) {
      wasPaused = true
      deps.pauseForRunaway()
      throw new ZenCustomRefusal('sending_off', ZEN_PAUSED_MESSAGE)
    }
    posts.push({ at: now, key })
  }

  /** Every Thread post and card answer is marked with where the widget
   *  sits (Garden + placement) and the hash its frame runs (0.46.1). The
   *  frame never sees or sets this: it comes from the port the message
   *  arrived on, so a widget can't drop or forge it, and the daemon holding
   *  the Garden checks the placement's approved `thread:post`. */
  const origin = (): { widget: string; garden: string; placement: string; hash: string } => {
    const w = deps.widget()
    return { widget: w.widget, garden: deps.gardenId(), placement: w.id, hash: deps.frameHash?.() || w.hash }
  }

  // ── k2.layout.* (prd-garden-column-collapse A1, CA10) ───────────────────
  // Frame-only, no cap: the caller's own page only, each verb at most 4 a
  // second (a refusal counts toward UW29), and a fold or unfold only within
  // 5 s of the person's press in this frame. That press is what lets the page
  // save a widget's fold like the person's own click (0.46.4).

  const layoutCalls = new Map<string, number[]>()

  // ── k2.open / links.open (0.46.4) ───────────────────────────────────────
  // Only just after the person's press in this frame, once a second and 10
  // a minute (a refusal counts toward UW29), one K2 card at a time.
  const linkCalls: number[] = []
  let linkAsking = false

  const openLink = async (args: unknown[]): Promise<{ ok: boolean }> => {
    const links = deps.links
    if (!links) refuse('verb_unavailable', 'links.open isn’t available here.')
    const target = zenLinkTarget(args[0])
    if (typeof target === 'string') refuse('bad_args', `links.open: ${target}`)
    const now = deps.now()
    while (linkCalls.length > 0 && now - linkCalls[0] >= 60_000) linkCalls.shift()
    const lastSecond = linkCalls.filter((t) => now - t < 1_000).length
    if (lastSecond >= ZEN_LINKS_OPEN.perSecond || linkCalls.length >= ZEN_LINKS_OPEN.perMinute) {
      refuse('rate_limited', `links.open: at most ${ZEN_LINKS_OPEN.perSecond} a second and ${ZEN_LINKS_OPEN.perMinute} a minute.`)
    }
    if (!links!.pressed()) refuse('failed', 'links.open needs a click: it works only just after the person presses in the widget.')
    if (linkAsking) refuse('rate_limited', 'links.open: K2 is already asking the person about a link.')
    linkCalls.push(now)
    const t = target as { url: string; site: string }
    linkAsking = true
    try {
      return { ok: await links!.open(t.url, t.site) }
    } finally {
      linkAsking = false
    }
  }

  const needLayout = (verb: string): ZenCustomLayout => {
    if (!deps.layout) refuse('verb_unavailable', `${verb} isn’t available here.`)
    const now = deps.now()
    const recent = (layoutCalls.get(verb) ?? []).filter((t) => now - t < 1_000)
    if (recent.length >= ZEN_LAYOUT_PER_SECOND) {
      layoutCalls.set(verb, recent)
      refuse('rate_limited', `${verb}: at most ${ZEN_LAYOUT_PER_SECOND} a second.`)
    }
    recent.push(now)
    layoutCalls.set(verb, recent)
    return deps.layout!
  }

  const needColumn = (verb: string, layout: ZenCustomLayout, v: unknown): ZenLayoutColumn => {
    const columns = layout.columns()
    if (typeof v !== 'number' || !Number.isInteger(v) || v < 0 || v >= columns.length) {
      refuse('bad_args', `${verb}: column is a column number, 0 to ${Math.max(0, columns.length - 1)}`)
    }
    return columns[v as number]
  }

  /** set / toggle answer the column as it is now (the catalog's LayoutColumn). */
  const layoutFold = (verb: string, layout: ZenCustomLayout, col: ZenLayoutColumn, collapsed: boolean): ZenLayoutColumn => {
    if (!layout.pressed()) refuse('failed', `${verb} needs a click: it works only just after the person presses in the widget.`)
    const refusal = layout.setByWidget(col.index, collapsed)
    if (refusal === 'bad_index') refuse('bad_args', `${verb}: no column ${col.index}`)
    if (refusal === 'fixed') refuse('failed', `${col.label} can’t fold: it isn’t collapsible.`)
    if (refusal === 'last_open') refuse('failed', `${col.label} is the only open column: it can’t fold.`)
    // The page redraws on its next render; the answer is where it goes.
    return { ...col, collapsed }
  }

  const inner = (verb: string, ...args: unknown[]): unknown => {
    try {
      return deps.inner.call(verb as never, ...args)
    } catch (err) {
      throw toRefusal(err)
    }
  }

  // ── Garden blobs and downloads (prd-garden-board-needs-v1 §3–§4) ─────────

  const needBlobs = (verb: string): ZenCustomBlobs => {
    if (!deps.blobs) refuse('verb_unavailable', `${verb} isn’t available here.`)
    return deps.blobs!
  }

  /** An options object with only `allowed` string fields. */
  const stringOpts = (verb: string, v: unknown, allowed: readonly string[]): Record<string, string> => {
    if (v === undefined || v === null) return {}
    if (!isObj(v)) refuse('bad_args', `${verb}: options are an object`)
    const out: Record<string, string> = {}
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) {
      if (!allowed.includes(k)) refuse('bad_args', `${verb}: unknown option '${k}'`)
      if (x === undefined || x === null) continue
      if (typeof x !== 'string' || x.length > 255) refuse('bad_args', `${verb}: ${k} is text (at most 255 characters)`)
      out[k] = x as string
    }
    return out
  }

  const blobCall = async <T,>(fn: () => Promise<T>): Promise<T> => {
    try {
      return await fn()
    } catch (err) {
      throw err instanceof ZenCustomRefusal ? err : toRefusal(zenStoreErrorFrom(err))
    }
  }

  const putBlob = async (args: unknown[], bytes: ArrayBuffer | undefined): Promise<unknown> => {
    const blobs = needBlobs('blobs.put')
    if (!bytes) refuse('bad_args', 'blobs.put: send the data as bytes (k2.blobs.put does).')
    if (args[0] !== undefined && args[0] !== null) refuse('bad_args', 'blobs.put: the data rides as bytes, not in the arguments.')
    const o = stringOpts('blobs.put', args[1], ['mime', 'name'])
    return blobCall(() => blobs.put(bytes!, o))
  }

  const getBlob = async (args: unknown[]): Promise<ZenBytesValue> => {
    const blobs = needBlobs('blobs.get')
    const ref = args[0]
    if (typeof ref !== 'string' || !ZEN_BLOB_REF.test(ref)) refuse('bad_args', 'blobs.get needs a ref (k2blob:sha256-<64 lowercase hex>)')
    const o = args[1]
    let px: number | undefined
    let from: string | undefined
    if (o !== undefined && o !== null) {
      if (!isObj(o) || Object.keys(o).some((k) => k !== 'px' && k !== 'from')) refuse('bad_args', 'blobs.get options are {px?, from?}')
      const p = (o as Record<string, unknown>).px
      if (p !== undefined && p !== null && p !== 256 && p !== 512) refuse('bad_args', 'blobs.get: px is 256 or 512')
      px = (p ?? undefined) as number | undefined
      const f = (o as Record<string, unknown>).from
      if (f !== undefined && f !== null && (typeof f !== 'string' || !f.trim())) refuse('bad_args', 'blobs.get: from is a widget name')
      from = (f ?? undefined) as string | undefined
    }
    const r = await blobCall(() => blobs.get(ref as string, { px, from }))
    return new ZenBytesValue({ ref: r.info.ref, size: r.info.size, mime: r.info.mime, name: r.info.name }, r.bytes)
  }

  const downloadCalls: number[] = []
  let downloading = false
  let ticket: { id: string; name: string; at: number } | null = null

  const needDownloads = (verb: string): ZenCustomDownloads => {
    if (!deps.downloads) refuse('verb_unavailable', `${verb} isn’t available here.`)
    return deps.downloads!
  }

  /** GS2: a press in this frame within 5 s, once every 2 s, 10 a minute,
   *  one at a time (a refusal counts toward UW29). */
  const downloadGate = (verb: string, d: ZenCustomDownloads): void => {
    const now = deps.now()
    while (downloadCalls.length > 0 && now - downloadCalls[0] >= 60_000) downloadCalls.shift()
    const last = downloadCalls[downloadCalls.length - 1]
    if ((last !== undefined && now - last < ZEN_DOWNLOADS.everyMs) || downloadCalls.length >= ZEN_DOWNLOADS.perMinute) {
      refuse('rate_limited', `${verb}: at most one download every 2 seconds and ${ZEN_DOWNLOADS.perMinute} a minute.`)
    }
    if (!d.pressed()) refuse('failed', `${verb} needs a click: it works only just after the person presses in the widget.`)
    if (downloading) refuse('rate_limited', `${verb}: a download is already running.`)
    downloadCalls.push(now)
  }

  const downloadName = (verb: string, raw: unknown, mime: unknown): string => {
    const r = zenDownloadName(raw, mime)
    if ('problem' in r) refuse('bad_args', `${verb}: ${r.problem}`)
    return (r as { name: string }).name
  }

  const ticketId = (): string => {
    const b = new Uint8Array(12)
    crypto.getRandomValues(b)
    return `dl-${Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('')}`
  }

  /** GS7: the press is checked now; the bytes may follow within 60 s. */
  const beginDownload = (args: unknown[]): { ticket: string } => {
    const d = needDownloads('downloads.begin')
    const o = stringOpts('downloads.begin', args[1], ['mime'])
    const name = downloadName('downloads.begin', args[0], o.mime)
    const now = deps.now()
    if (ticket && now - ticket.at <= ZEN_DOWNLOADS.ticketMs) refuse('rate_limited', 'downloads.begin: a download is already waiting for its bytes.')
    downloadGate('downloads.begin', d)
    ticket = { id: ticketId(), name, at: now }
    return { ticket: ticket.id }
  }

  const saveDownload = async (args: unknown[], bytes: ArrayBuffer | undefined): Promise<{ saved: boolean; name?: string }> => {
    const d = needDownloads('downloads.save')
    const o = stringOpts('downloads.save', args[2], ['mime', 'ticket'])
    const name = downloadName('downloads.save', args[0], o.mime)
    if (o.ticket !== undefined) {
      const t = ticket
      const now = deps.now()
      if (t && now - t.at > ZEN_DOWNLOADS.ticketMs) ticket = null
      if (!t || t.id !== o.ticket || now - t.at > ZEN_DOWNLOADS.ticketMs) {
        refuse('failed', 'downloads.save: start the download from a click (this ticket is unknown, used, or older than 60 s).')
      }
      if (t!.name !== name) refuse('bad_args', `downloads.save: the ticket is for ${t!.name}`)
      // One ticket, one file.
      ticket = null
      if (downloading) refuse('rate_limited', 'downloads.save: a download is already running.')
    } else {
      downloadGate('downloads.save', d)
    }
    const data = args[1]
    let read: DownloadSliceReader
    if (bytes) {
      if (data !== undefined && data !== null) refuse('bad_args', 'downloads.save: the data rides as bytes, not in the arguments.')
      read = zenBytesReader(bytes)
    } else if (isObj(data) && Object.keys(data).length === 1 && typeof data.blob === 'string' && ZEN_BLOB_REF.test(data.blob)) {
      // GS4: a file this widget's data holds goes daemon → Downloads; the
      // daemon checks store:read and the maker's rights on every slice.
      if (!caps().has('store:read')) refuse('cap_not_granted', 'downloads.save with {blob} needs store:read.', { cap: 'store:read' })
      read = needBlobs('downloads.save').reader(data.blob)
    } else {
      refuse('bad_args', 'downloads.save takes bytes (k2.download sends them) or {blob: ref}.')
    }
    downloading = true
    try {
      const landed = await d.save(name, read!)
      if (landed === null) return { saved: false }
      return { saved: true, name: landed.split(/[\\/]/).pop() || name }
    } finally {
      downloading = false
    }
  }

  // ── calls ────────────────────────────────────────────────────────────────

  const calls: Record<string, (args: unknown[], bytes?: ArrayBuffer) => unknown> = {
    // prd-garden-board-needs-v1: files in this widget's data, and downloads.
    'blobs.put': (args, bytes) => putBlob(args, bytes),
    'blobs.get': (args) => getBlob(args),
    'downloads.begin': (args) => beginDownload(args),
    'downloads.save': (args, bytes) => saveDownload(args, bytes),
    'gardens.list': () => inner('gardens.list'),
    'gardens.current': () => inner('gardens.current'),
    'gardens.switch': (args) => {
      const now = deps.now()
      if (now - mountedAt < ZEN_WIDGET_BUDGET.switchAfterMountMs || now - lastSwitch < ZEN_WIDGET_BUDGET.switchEveryMs) {
        refuse('rate_limited', 'gardens.switch: at most once every 2 seconds, and not just after the page opens.')
      }
      const id = args[0]
      const list = inner('gardens.list') as Array<{ id: string }>
      if (typeof id !== 'string' || !list.some((g) => g.id === id)) refuse('failed', 'No such Garden.')
      lastSwitch = now
      return inner('gardens.switch', id)
    },
    // ThemeInfo {theme: {scheme, vars}, chrome: {corners, stoplights}, motion}.
    'theme.get': () => deps.theme(),
    // The whole canvas (`zen-canvas`): no data, no cap.
    'canvas.setHitRegions': (args) => {
      if (!deps.canvas) refuse('verb_unavailable', 'canvas.setHitRegions isn’t available here.')
      deps.canvas!.setHitRegions(zenHitRegionsFrom(args[0]))
      return null
    },
    'window.startDrag': () => {
      if (!deps.canvas) refuse('verb_unavailable', 'window.startDrag isn’t available here.')
      deps.canvas!.startDrag()
      return null
    },
    // 0.46.4: k2.open(url), the person's browser (K2's card the first time per site).
    'links.open': (args) => openLink(args),
    // A1: the page's columns: [{index, label, collapsible, collapsed}].
    'layout.get': () => ({ columns: needLayout('layout.get').columns() }),
    'layout.set': (args) => {
      const layout = needLayout('layout.set')
      const o = args[0]
      if (!isObj(o) || Object.keys(o).some((k) => k !== 'column' && k !== 'collapsed') || typeof o.collapsed !== 'boolean') {
        refuse('bad_args', 'layout.set takes {column, collapsed}')
      }
      const r = o as { column: unknown; collapsed: boolean }
      return layoutFold('layout.set', layout, needColumn('layout.set', layout, r.column), r.collapsed)
    },
    'layout.toggle': (args) => {
      const layout = needLayout('layout.toggle')
      const col = needColumn('layout.toggle', layout, args[0])
      return layoutFold('layout.toggle', layout, col, !col.collapsed)
    },
    'agents.list': () => projectZenRows(inner('agents.list') as ZenAgentRow[], caps()),
    'conversation.open': async (args) => {
      // In Zen only: `where: "agents"` is never passed through (UW16).
      const address = needAddress('conversation.open', args[0])
      await inner('conversation.open', address)
      return null
    },
    'conversation.close': () => {
      inner('conversation.close')
      return null
    },
    'presence.get': (args) => projectZenPeople(inner('presence.get', needAddress('presence.get', args[0])) as ZenPerson[]),
    'thread.read': async (args) => {
      const address = needAddress('thread.read', args[0])
      const o = isObj(args[1]) ? args[1] : {}
      const limit = typeof o.limit === 'number' ? Math.min(100, Math.max(1, Math.floor(o.limit))) : undefined
      // thread-delta-v1: afterSeq, never with beforeSeq.
      if (o.afterSeq !== undefined) {
        const afterSeq = needInt('thread.read', 'afterSeq', o.afterSeq, 0)
        if (o.beforeSeq !== undefined) refuse('bad_args', 'thread.read: afterSeq or beforeSeq, not both')
        await deps.ensureConversation(address)
        const d = (await inner('thread.read', address, { afterSeq, ...(limit !== undefined ? { limit } : {}) })) as ZenThreadDelta
        return projectZenThreadDelta(d)
      }
      await deps.ensureConversation(address)
      const opts = typeof o.beforeSeq === 'number' ? { beforeSeq: o.beforeSeq } : limit !== undefined ? { limit } : {}
      const r = (await inner('thread.read', address, opts)) as ZenThreadView | { items: OverlayThreadItem[]; hasMore: boolean }
      if ('phase' in r) return projectZenThreadView(r)
      return { items: projectZenThreadItems(r.items), hasMore: r.hasMore }
    },
    'thread.post': async (args) => {
      const address = needAddress('thread.post', args[0])
      const text = needText('thread.post', args[1])
      // UW19, UW49: never paths or uploads. The only option is
      // `attachments`: workspace-relative files the person picked
      // (`thread.pickFiles`), checked here before anything is sent.
      let attachments: ReturnType<typeof zenThreadAttachments> = []
      const o = args[2]
      if (args.length > 2 && o !== undefined && o !== null) {
        if (!isObj(o) || Object.keys(o).some((k) => k !== 'attachments')) {
          refuse('failed', 'Widgets never send paths or files; attach what the person picks with thread.pickFiles.')
        }
        const raw = (o as Record<string, unknown>).attachments
        if (raw !== undefined) {
          try {
            attachments = zenThreadAttachments(raw)
          } catch (err) {
            throw toRefusal(err)
          }
        }
      }
      if (!text.trim() && attachments.length === 0) refuse('failed', 'Nothing to send.')
      needSending()
      guard(address, attachments.length > 0 ? `${text}\n${attachments.map((a) => a.relPath).join('\n')}` : text)
      await deps.ensureConversation(address)
      const r = (await inner('thread.post', address, text, {
        origin: origin(),
        ...(attachments.length > 0 ? { attachments } : {}),
      })) as { id: string | null; seq: number | null }
      return { id: r.id, seq: r.seq }
    },
    'thread.markRead': async (args) => {
      const address = needAddress('thread.markRead', args[0])
      const o = args[1]
      if (o !== undefined && o !== null && !isObj(o)) refuse('bad_args', 'thread.markRead takes {seq?}')
      const seq = isObj(o) && o.seq !== undefined ? needInt('thread.markRead', 'seq', o.seq, 0) : undefined
      await deps.ensureConversation(address)
      const r = (await inner('thread.markRead', address, seq !== undefined ? { seq } : {})) as Record<string, unknown> | null
      const n = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)
      return { changed: r?.changed === true, lastReadSeq: n(r?.lastReadSeq), lastSeq: n(r?.lastSeq), unread: n(r?.unread) }
    },
    'thread.attachment': async (args) => {
      const address = needAddress('thread.attachment', args[0])
      if (typeof args[1] !== 'string' || !args[1]) refuse('bad_args', 'thread.attachment needs a message id')
      const n = needInt('thread.attachment', 'n', args[2], 0)
      const o = args[3]
      if (o !== undefined && o !== null && !isObj(o)) refuse('bad_args', 'thread.attachment takes {mode?, px?}')
      const opts: { mode?: string; px?: number } = {}
      if (isObj(o)) {
        if (Object.keys(o).some((k) => k !== 'mode' && k !== 'px')) refuse('bad_args', 'thread.attachment takes {mode?, px?}')
        if (o.mode !== undefined) {
          if (o.mode !== 'meta' && o.mode !== 'preview' && o.mode !== 'thumb') refuse('bad_args', 'mode is meta, preview or thumb')
          opts.mode = o.mode as string
        }
        if (o.px !== undefined) {
          const px = needInt('thread.attachment', 'px', o.px, 64)
          if (px > 512) refuse('bad_args', 'thread.attachment: px is 64 to 512')
          opts.px = px
        }
      }
      await deps.ensureConversation(address)
      return projectZenThreadFileData(await inner('thread.attachment', address, args[1], n, opts))
    },
    'thread.open': async (args) => {
      const address = needAddress('thread.open', args[0])
      const t = args[1]
      if (!isObj(t)) refuse('bad_args', 'thread.open takes {docId, n} or {rel, line?}')
      const target = t as Record<string, unknown>
      let clean: Record<string, unknown>
      if (typeof target.docId === 'string' && target.docId) {
        clean = { docId: target.docId, n: needInt('thread.open', 'n', target.n, 0) }
      } else if (typeof target.rel === 'string' && target.rel.trim() && target.rel.length <= 4096) {
        clean = { rel: target.rel, ...(target.line !== undefined ? { line: needInt('thread.open', 'line', target.line, 1) } : {}) }
      } else {
        return refuse('bad_args', 'thread.open takes {docId, n} or {rel, line?}')
      }
      await deps.ensureConversation(address)
      const r = (await inner('thread.open', address, clean)) as { opened?: unknown } | null
      return { opened: r?.opened === true }
    },
    'thread.links': async (args) => {
      const address = needAddress('thread.links', args[0])
      const list = args[1]
      if (!Array.isArray(list) || list.some((c) => typeof c !== 'string' || c.length > 4096)) {
        refuse('bad_args', 'thread.links takes a list of path-shaped strings')
      }
      if ((list as string[]).length > 200) refuse('bad_args', 'thread.links takes up to 200 candidates')
      await deps.ensureConversation(address)
      const r = (await inner('thread.links', address, [...(list as string[])])) as { links?: unknown } | null
      const links = Array.isArray(r?.links) ? r.links : []
      return {
        // Built key by key: the host path and root never cross.
        links: links.filter(isObj).map((a) => {
          const out: { input: string; linkable: boolean; kind?: string; rel?: string } = {
            input: typeof a.input === 'string' ? a.input : '',
            linkable: a.linkable === true && typeof a.rel === 'string',
          }
          if (out.linkable) {
            out.rel = a.rel as string
            if (typeof a.kind === 'string') out.kind = a.kind
          }
          return out
        }),
      }
    },
    'thread.pickFiles': async (args) => {
      const address = needAddress('thread.pickFiles', args[0])
      if (deps.gesture && !deps.gesture()) refuse('failed', 'thread.pickFiles opens K2’s picker only when the person clicks in the widget.')
      if (picking) refuse('rate_limited', 'K2’s file picker is already open.')
      picking = true
      try {
        await deps.ensureConversation(address)
        const r = (await inner('thread.pickFiles', address)) as { files?: unknown } | null
        const files = Array.isArray(r?.files) ? r.files : []
        return {
          files: files.filter(isObj).flatMap((f) => {
            if (typeof f.relPath !== 'string' || !f.relPath) return []
            const out: { relPath: string; name: string; size?: number; mime?: string } = {
              relPath: f.relPath,
              name: typeof f.name === 'string' && f.name ? f.name : f.relPath.split('/').pop() || f.relPath,
            }
            if (typeof f.size === 'number' && Number.isFinite(f.size)) out.size = f.size
            if (typeof f.mime === 'string' && f.mime) out.mime = f.mime
            return [out]
          }),
        }
      } finally {
        picking = false
      }
    },
    'thread.answer': async (args) => {
      const address = needAddress('thread.answer', args[0])
      const cardId = args[1]
      if (typeof cardId !== 'string' || !cardId) refuse('failed', 'thread.answer needs a card id')
      // UW50: a choice only, never a secret.
      if (typeof args[2] !== 'string') refuse('failed', 'Widgets answer with a choice only, never a secret.')
      needText('thread.answer', args[2])
      needSending()
      // Permission cards: only a person answers, on K2's sheet outside the
      // frame. The widget hears `null` once the person closed it.
      if (deps.permissions?.handles(address, cardId as string)) {
        const opened = await deps.permissions.open(address, cardId as string)
        if (!opened.ok) refuse(opened.code, opened.message)
        return null
      }
      await deps.ensureConversation(address)
      await inner('thread.answer', address, cardId, { answer: args[2], origin: origin() })
      return null
    },
    'compose.draft': (args) => {
      const address = needAddress('compose.draft', args[0])
      inner('compose.draft', address, needText('compose.draft', args[1]))
      return null
    },
    // ── Garden columns and the agent list (catalog v20, 0.46.3 r10) ────
    // Day-0 stubs (prd-garden-column-collapse-and-agent-list-v1 §7): each
    // lane fills its own body (layout.* is lane A's, above):
    // agents.show (lane B), conversation.setContext (lane C).
    // Lane B (CA11): full row addresses exactly as agents.list() gives them
    // (a bare handle is refused), at most 50; null clears. K2 draws the
    // rows in the Agents widget whose `list-from` names this widget.
    'agents.show': (args) => {
      const v = args[0]
      let list: string[] | null = null
      if (v !== null && v !== undefined) {
        if (!Array.isArray(v)) refuse('bad_args', 'agents.show takes a list of addresses from agents.list(), or null.')
        const arr = v as unknown[]
        if (arr.length > ZEN_AGENTS_SHOW_MAX) refuse('bad_args', `agents.show takes at most ${ZEN_AGENTS_SHOW_MAX} addresses.`)
        const reach = new Set(deps.boundRows().map((r) => r.address))
        const seen = new Set<string>()
        for (const a of arr) {
          if (typeof a !== 'string') refuse('bad_args', 'agents.show takes addresses (text), from agents.list().')
          const address = a as string
          if (!reach.has(address)) {
            refuse('not_bound', `${address} isn’t one of this widget’s agents (use a full address from agents.list()).`, {
              room: handleOf(address),
            })
          }
          if (seen.has(address)) refuse('bad_args', `${address} is listed twice.`)
          seen.add(address)
        }
        list = arr as string[]
      }
      const release = inner('agents.show', list)
      // Sending the same list again keeps the same list (K2 does nothing).
      unshow = typeof release === 'function' ? (release as () => void) : null
      return null
    },
    // §4 (lane C): the Board context chip. Visible and removable in every
    // Conversation that follows this widget; it rides only the person's own
    // send, as K2's quoted line (the daemon checks it again).
    'conversation.setContext': (args) => {
      const now = deps.now()
      while (contextCalls.length > 0 && now - contextCalls[0] >= 1000) contextCalls.shift()
      if (contextCalls.length >= ZEN_THREAD_CONTEXT_PER_SECOND) {
        refuse('rate_limited', `conversation.setContext: at most ${ZEN_THREAD_CONTEXT_PER_SECOND} calls a second.`)
      }
      contextCalls.push(now)
      const text = args[0]
      if (text === null || text === undefined) {
        if (contextSet) inner('conversation.setContext', null)
        contextSet = false
        return null
      }
      if (typeof text !== 'string') refuse('bad_args', 'conversation.setContext takes one line of text, or null')
      const problem = zenThreadContextProblem(text as string)
      if (problem) refuse('bad_args', problem)
      inner('conversation.setContext', text, origin())
      contextSet = true
      return null
    },
    // ── Garden v3 data sources (contract-sources-v1 §6) ────────────────
    'sources.list': () => {
      if (!deps.sources) refuse('verb_unavailable', 'sources.list isn’t available here.')
      return deps.sources!.reader.list(deps.sources!.bound(), deps.gardenId())
    },
    'sources.read': (args) => {
      const { address, src } = needSource('sources.read', args[0])
      return sourceCall(address, () => src.reader.read(address, deps.gardenId()))
    },
    'sources.history': (args) => {
      const { address, src } = needSource('sources.history', args[0])
      const field = args[1]
      if (typeof field !== 'string' || !field) refuse('failed', 'sources.history needs a field name')
      const o = isObj(args[2]) ? args[2] : {}
      const opts: { fromAt?: number; toAt?: number } = {}
      if (typeof o.fromAt === 'number' && Number.isFinite(o.fromAt)) opts.fromAt = Math.floor(o.fromAt)
      if (typeof o.toAt === 'number' && Number.isFinite(o.toAt)) opts.toAt = Math.floor(o.toAt)
      return sourceCall(address, () => src.reader.history(address, field as string, opts, deps.gardenId()))
    },
    'sources.refresh': (args) => {
      const { address, src } = needSource('sources.refresh', args[0])
      return sourceCall(address, async () => {
        const r = await src.reader.refresh(address, deps.gardenId())
        return { queued: r.queued, nextAt: r.nextAt ?? null }
      })
    },
    // K2's own Ask box: the widget supplies no text (works with or without thread:post).
    // Retired in 0.46.4 (Rosson 2026-10-23: Ask an agent wasn't useful): the
    // cap and the address are still checked, then it answers `retired` and
    // opens nothing; the row goes in 0.47.0 (retirement ledger).
    'sources.ask': (args) => {
      needSource('sources.ask', args[0])
      return refuse('retired', ZEN_SOURCES_ASK_RETIRED)
    },
  }

  // ── Garden data (contract-garden-data-v1 §7): the daemon checks the
  // placement, hash, caps and the maker's rights on every call ──────────

  const needStore = (verb: string): ZenCustomStore => {
    if (!deps.store) refuse('verb_unavailable', `${verb} isn’t available here.`)
    return deps.store!
  }
  for (const verb of ZEN_STORE_VERBS) {
    calls[verb] = async (args) => {
      const store = needStore(verb)
      try {
        return await store.call(verb, args)
      } catch (err) {
        throw toRefusal(err)
      }
    }
  }

  // ── RS-A15 guest rooms: the daemon checks everything; a write waits for
  // the person on K2's sheet (the frame only hears the answer) ───────────

  for (const verb of ZEN_GUEST_VERBS) {
    calls[verb] = async (args) => {
      if (!deps.guests) refuse('verb_unavailable', `${verb} isn’t available here.`)
      try {
        return await deps.guests!.call(verb, args)
      } catch (err) {
        throw toRefusal(err)
      }
    }
  }

  // ── subscriptions ────────────────────────────────────────────────────────

  const subscribers: Record<
    string,
    (args: unknown[], push: (v: unknown) => void, fail: (error: CatalogWireError) => void) => () => void
  > = {
    'theme.changed': (_args, push) => deps.onThemeChange(() => push(deps.theme())),
    // A1: the columns now, then again whenever one folds or opens (any
    // cause). Counts toward the 16 live subscriptions.
    'layout.subscribe': (_args, push) => {
      const layout = needLayout('layout.subscribe')
      const off = layout.subscribe((columns) => push({ columns }))
      push({ columns: layout.columns() })
      return off
    },
    'agents.subscribe': (_args, push) => {
      let last = ''
      return inner('agents.subscribe', (rows: ZenAgentRow[]) => {
        const projected = projectZenRows(rows, caps())
        const sig = JSON.stringify(projected)
        if (sig === last) return
        last = sig
        push(projected)
      }) as () => void
    },
    'presence.subscribe': (args, push) => {
      const address = needAddress('presence.subscribe', args[0])
      return inner('presence.subscribe', address, (people: ZenPerson[]) => push(projectZenPeople(people))) as () => void
    },
    'thread.subscribe': (args, push) => {
      const address = needAddress('thread.subscribe', args[0])
      let last = ''
      const off = inner('thread.subscribe', address, (view: ZenThreadView) => {
        const projected = projectZenThreadView(view)
        const sig = JSON.stringify(projected)
        if (sig === last) return
        last = sig
        push(projected)
      }) as () => void
      void deps.ensureConversation(address).catch((err: unknown) => console.warn(`[zen] widget Thread for ${address}:`, err))
      return off
    },
    // 0.46.5 activity SDK: the Thread turn as an App guest sees it, on the
    // same feed as thread.subscribe (K2's socket, the catch-up read on every
    // (re)open, a re-read on focus). The current turn first (an ended one
    // reads as none, like thread.activity), then every change.
    'thread.onActivity': (args, push) => {
      const address = needAddress('thread.onActivity', args[0])
      let last = ''
      let endedBefore: string | null | undefined
      const off = inner('thread.subscribe', address, (view: ZenThreadView) => {
        if (endedBefore === undefined) endedBefore = zenEndedAtSubscribe(view)
        const projected = projectZenActivityState(view, endedBefore)
        const sig = JSON.stringify(projected)
        if (sig === last) return
        last = sig
        push(projected)
      }) as () => void
      void deps.ensureConversation(address).catch((err: unknown) => console.warn(`[zen] widget Thread for ${address}:`, err))
      return off
    },
    // app-room-stream (0.46.3): every new message on this server's agents,
    // one socket for all of them; the item is projected like a Thread view's.
    'thread.inbox': (_args, push) =>
      inner('thread.inbox', (m: ZenInboxMessage) => {
        const item = projectZenThreadItem(m.item)
        if (item) push({ address: m.address, ...item })
      }) as () => void,
    // Garden data: `k2.store.subscribe(prefix, cb)` subscribes on the
    // `store.changes` row; pushes whole revs from any writer.
    [ZEN_STORE_SUBSCRIBE_VERB]: (args, push, fail) => {
      const store = needStore('store.subscribe')
      try {
        return store.subscribe(args[0], push, fail)
      } catch (err) {
        throw toRefusal(err)
      }
    },
    // Garden v3: the card view, pushed on every change; an unreachable server
    // pushes the last view greyed (reach "unreachable", phase "stale").
    'sources.subscribe': (args, push, fail) => {
      const { address, src } = needSource('sources.subscribe', args[0])
      let last = ''
      try {
        return src.reader.subscribe(
          address,
          (view) => {
            const sig = JSON.stringify(view)
            if (sig === last) return
            last = sig
            push(view)
          },
          (err) => fail(sourceRefusal(address, err).wire),
          deps.gardenId(),
        )
      } catch (err) {
        throw sourceRefusal(address, err)
      }
    },
  }

  const lookup = (verb: unknown): { verb: string; row: Row } => {
    if (typeof verb !== 'string') refuse('unknown_verb', 'A verb is a string.')
    const v = verb as string
    const row = Object.prototype.hasOwnProperty.call(CUSTOM, v) ? CUSTOM[v] : undefined
    if (!row) {
      if (Object.prototype.hasOwnProperty.call(ZEN_VERBS, v)) refuse('not_exposed', `${v} isn’t available to custom widgets.`)
      refuse('unknown_verb', `No verb ${v}.`)
    }
    // 0.46.5: a row may need more than one cap (`alsoCaps`): each is
    // checked, and the refusal names the first one missing.
    const missingAlso = (row!.also ?? []).find((c) => !caps().has(c))
    if (row!.cap !== null && caps().has(row!.cap) && missingAlso) {
      refuse('cap_not_granted', `${v} needs ${missingAlso}.`, { cap: missingAlso })
    }
    if (row!.cap !== null && !caps().has(row!.cap)) {
      // CA14 (Garden access on): an unsigned version that asked for
      // thread:post lost it to the signer gate; only an editor's signature
      // gives it back. Off, `signed` is absent: today's cap check.
      const w = deps.widget()
      if (v === 'conversation.setContext' && w.signed === false && (w.requested as readonly string[]).includes(row!.cap)) {
        refuse('needs_editor', `${v} needs ${row!.cap}, which this unsigned version can’t use until an editor signs it.`, { cap: row!.cap })
      }
      refuse('cap_not_granted', `${v} needs ${row!.cap}.`, { cap: row!.cap })
    }
    return { verb: v, row: row! }
  }

  function dispose(): void {
    // §4: a widget that stops or unloads takes its Board context with it.
    if (contextSet) {
      contextSet = false
      clearZenThreadContextFor(deps.gardenId(), deps.widget().id)
    }
    // The list clears when the widget stops or unloads (CA11).
    if (unshow) {
      const release = unshow
      unshow = null
      try {
        release()
      } catch (err) {
        console.warn('[zen] clearing the widget’s agents.show list failed:', err)
      }
    }
    for (const sub of [...held.keys()]) dropHeld(sub)
    for (const s of subs.values()) {
      try {
        s.off()
      } catch (err) {
        console.warn('[zen] widget unsubscribe failed:', err)
      }
    }
    subs.clear()
  }

  const handle = async (msg: unknown): Promise<ZenFrameReply | null> => {
    if (stopped || !isObj(msg)) return null
    const m = msg as ZenFrameRequest & Record<string, unknown>
    const id = typeof m.id === 'number' ? m.id : null
    try {
      if (typeof m.unsub === 'number') {
        const s = subs.get(m.unsub)
        subs.delete(m.unsub)
        dropHeld(m.unsub)
        s?.off()
        return null
      }
      if (id === null && typeof m.sub !== 'number') return null
      // GB12 (V1): transferred bytes, which the JSON size check can't see.
      let bytes: ArrayBuffer | undefined
      if ('bytes' in m && m.bytes !== undefined) {
        const name = typeof m.verb === 'string' ? m.verb : ''
        const cap = Object.prototype.hasOwnProperty.call(ZEN_BYTE_VERBS, name) ? ZEN_BYTE_VERBS[name] : undefined
        if (cap === undefined || typeof m.sub === 'number') refuse('bad_args', `${name || 'This call'} takes no bytes.`)
        if (!isArrayBuffer(m.bytes)) refuse('bad_args', 'Bytes are one ArrayBuffer.')
        if ((m.bytes as ArrayBuffer).byteLength > cap!) refuse('too_large', `${name} takes at most ${cap! / (1024 * 1024)} MiB.`)
        bytes = m.bytes as ArrayBuffer
      }
      if (JSON.stringify(msg).length > ZEN_WIDGET_BUDGET.messageBytes) refuse('too_large', 'A message is over 64 KB.')
      takeToken()
      // 0.46.1: args are a list when given (bad_args, not a guess).
      if (m.args !== undefined && !Array.isArray(m.args)) refuse('bad_args', 'A call’s arguments are a list.')
      const args = Array.isArray(m.args) ? (m.args as unknown[]) : []
      const { verb, row } = lookup(m.verb)
      if (typeof m.sub === 'number') {
        const sub = m.sub
        if (row.kind === 'call' && verb !== ZEN_STORE_SUBSCRIBE_VERB) refuse('failed', `${verb} is a call: use k2.call.`)
        if (subs.has(sub)) return null
        const budget = verb !== 'theme.changed'
        const thread = verb === 'thread.subscribe' || verb === 'thread.inbox' || verb === 'thread.onActivity'
        const live = [...subs.values()]
        if (budget && live.filter((s) => s.budget).length >= ZEN_WIDGET_BUDGET.liveSubscriptions) {
          refuse('rate_limited', 'At most 16 live subscriptions.')
        }
        if (thread && live.filter((s) => s.thread).length >= ZEN_WIDGET_BUDGET.threadSubscriptions) {
          refuse('rate_limited', 'At most 16 Thread subscriptions.')
        }
        // Registered first: a subscriber may push its first value at once.
        const entry = { off: (): void => undefined, thread, budget }
        subs.set(sub, entry)
        const address = typeof args[0] === 'string' ? canonicalSourceAddress(args[0]) : null
        const start = (): void => {
          entry.off = subscribers[verb](
            args,
            (value) => {
              if (!stopped && subs.get(sub) === entry) deps.push(sub, zenPlain(value))
            },
            (error) => {
              // An async refusal ends the subscription (the runtime calls
              // onError), unless it is a `not_bound` source K2 holds for a
              // rebind (0.46.1).
              if (stopped || subs.get(sub) !== entry) return
              const wire = zenPlain(error) as CatalogWireError
              entry.off()
              entry.off = (): void => undefined
              if (canHold(verb, wire)) {
                hold(sub, address, wire, start)
                return
              }
              subs.delete(sub)
              deps.pushError?.(sub, wire)
            },
          )
        }
        try {
          start()
        } catch (err) {
          const wire = toRefusal(err).wire
          if (canHold(verb, wire)) {
            // Not answered yet: a placement change may bind it (boundChanged).
            hold(sub, address, wire, start)
            return null
          }
          subs.delete(sub)
          throw err
        }
        if (subs.get(sub) !== entry) entry.off()
        return null
      }
      if (row.kind !== 'call') refuse('failed', `${verb} is a subscription: use k2.subscribe.`)
      const value = await calls[verb](args, bytes)
      if (value instanceof ZenBytesValue) return { id: id as number, ok: true, value: zenPlain(value.value), bytes: value.bytes }
      return { id: id as number, ok: true, value: zenPlain(value) }
    } catch (err) {
      const wire = toRefusal(err).wire
      // A refused subscription is answered once (`{sub, error}`), never
      // dropped: the runtime ends it and tells the widget's onError.
      if (id === null) return typeof m.sub === 'number' ? { sub: m.sub, error: wire } : null
      return { id, ok: false, error: wire }
    }
  }

  /** The placement's addresses changed: start each held subscription whose
   *  address is bound now. Another refusal is answered at once. */
  const boundChanged = (): void => {
    if (stopped || held.size === 0 || !deps.sources) return
    const bound = new Set(deps.sources.bound().map((a) => canonicalSourceAddress(a)).filter((a): a is string => a !== null))
    for (const [sub, h] of [...held.entries()]) {
      if (h.address === null || !bound.has(h.address)) continue
      dropHeld(sub)
      if (!subs.has(sub)) continue
      try {
        h.start()
      } catch (err) {
        subs.delete(sub)
        deps.pushError?.(sub, toRefusal(err).wire)
      }
    }
  }

  return {
    handle,
    boundChanged,
    get subscriptions() {
      return subs.size
    },
    get held() {
      return held.size
    },
    dispose,
  }
}

/** Any thrown value as the shared error list (UWA10). */
export function toRefusal(err: unknown): ZenCustomRefusal {
  if (err instanceof ZenCustomRefusal) return err
  if (err instanceof ZenStoreError) {
    return new ZenCustomRefusal(err.wire.code, err.wire.message, err.wire.last !== undefined ? { last: err.wire.last } : {})
  }
  if (err instanceof ZenGuestsError) return new ZenCustomRefusal(err.wire.code, err.wire.message)
  if (err instanceof ZenCanvasError) return new ZenCustomRefusal(err.code, err.message)
  // 0.46.3 Thread parity verbs keep their code (bad_args, verb_unavailable, too_large…).
  if (err instanceof ZenThreadVerbError) return new ZenCustomRefusal(err.code, err.message)
  if (err instanceof DaemonHttpError && err.status === 413) return new ZenCustomRefusal('too_large', err.message)
  // The daemon's own `thread:post` check (0.46.1, `zen/widget/check`).
  if (err instanceof ZenWidgetError && err.code === 'widget_cap_missing') {
    return new ZenCustomRefusal('cap_not_granted', err.message, { cap: 'thread:post' })
  }
  if (err instanceof ZenBridgeError) {
    if (err.code === 'cap_not_granted') return new ZenCustomRefusal('cap_not_granted', err.message)
    if (err.code === 'verb_unavailable') return new ZenCustomRefusal('verb_unavailable', err.message)
    return new ZenCustomRefusal('failed', err.message)
  }
  return new ZenCustomRefusal('failed', err instanceof Error ? err.message : String(err))
}
