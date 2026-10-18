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
//   6. posting: text only, ≤ 4,000 characters, never files or secrets
//      (UW19, UW50); every post and card answer carries the widget's
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
//      a press in that frame.
// Then K2's own verb runs through the widget's inner bridge (built with the
// effective caps) and the answer is projected to the guest shape
// (`zen-custom-projection`) and cloned as plain JSON (functions dropped).
// Renderer-side checks are the second of three (UW26): the daemon decided
// which caps the placement may claim, and every server re-checks your role.

import { ZenBridgeError, ZEN_VERBS, type ZenWidgetBridge } from './zen-bridge'
import { ZEN_CUSTOM_VERBS } from './zen-verbs.generated'
import { USER_WIDGET_CAPS, type UserWidgetCap } from '../k2-caps.generated'
import type { CatalogErrorCode, CatalogWireError } from '../contract/catalog-types'
import type { ZenCustomWidgetPayload, ZenFrameReply, ZenFrameRequest } from './zen-custom-types'
import type { ZenAgentRow, ZenPerson, ZenThreadView } from './zen-data'
import {
  projectZenPeople,
  projectZenRows,
  projectZenThreadItems,
  projectZenThreadView,
} from './zen-custom-projection'
import type { ZenWidgetStopReason } from './zen-custom-run'
import { ZenCanvasError, zenHitRegionsFrom } from './zen-canvas'
import type { ZenRect } from './zen-controls'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { canonicalSourceAddress, ZenSourceError, type ZenSourcesReader } from './zen-sources'
import { ZEN_STORE_VERBS, ZenStoreError, type ZenCustomStore } from './zen-custom-store'
import { ZEN_GUEST_VERBS, ZenGuestsError, type ZenCustomGuests } from './zen-custom-guests'
import { ZenWidgetError } from './zen-widget-routes'

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
  /** Stop this widget (its frame goes; K2 draws the card). */
  stop(reason: ZenWidgetStopReason): void
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
  now(): number
}

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

/** How long a `not_bound` `sources.subscribe` waits for the placement to
 *  bind its address before the refusal reaches the widget (0.46.1 S3). A
 *  refused subscription ends in the frame (its onError), so K2 holds it
 *  instead and starts it if `boundChanged` brings the address in time. */
export const ZEN_SOURCES_REBIND_MS = 30_000

type Row = { cap: UserWidgetCap | null; kind: 'call' | 'subscribe' | 'event' }

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
 * `source: user` whatever the body said). **Static taint (GV48):** a widget
 * that asks for `sources:read` never runs with `thread:post`, so no
 * `thread.post`, `thread.answer` or `compose.draft` (the daemon's
 * `effective_caps` drops it too). K2's own `sources.ask` still works.
 * **Garden data:** it never runs with `store:write` either (it could copy
 * source values into data an agent can read); `store:read`, `save.*` and
 * `viewer.get` still work (contract-garden-data-v1 §7.1 step 4).
 */
export const ZEN_TAINTED_BY_SOURCES: readonly string[] = ['thread:post', 'store:write']

export function zenEffectiveCaps(w: Pick<ZenCustomWidgetPayload, 'caps' | 'requested'>): ReadonlySet<string> {
  const requested = w.requested as readonly string[]
  const readsSources = requested.includes(SOURCES_READ_CAP)
  return new Set(
    (w.caps as readonly string[]).filter(
      (c) => WIDGET_CAPS.has(c) && requested.includes(c) && !(readsSources && ZEN_TAINTED_BY_SOURCES.includes(c)),
    ),
  )
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

  const inner = (verb: string, ...args: unknown[]): unknown => {
    try {
      return deps.inner.call(verb as never, ...args)
    } catch (err) {
      throw toRefusal(err)
    }
  }

  // ── calls ────────────────────────────────────────────────────────────────

  const calls: Record<string, (args: unknown[]) => unknown> = {
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
      await deps.ensureConversation(address)
      const opts = typeof o.beforeSeq === 'number' ? { beforeSeq: o.beforeSeq } : limit !== undefined ? { limit } : {}
      const r = (await inner('thread.read', address, opts)) as ZenThreadView | { items: OverlayThreadItem[]; hasMore: boolean }
      if ('phase' in r) return projectZenThreadView(r)
      return { items: projectZenThreadItems(r.items), hasMore: r.hasMore }
    },
    'thread.post': async (args) => {
      const address = needAddress('thread.post', args[0])
      const text = needText('thread.post', args[1])
      if (!text.trim()) refuse('failed', 'Nothing to send.')
      // UW19, UW49: text only. Any options (paths, files) are refused
      // before anything is read or uploaded.
      if (args.length > 2 && args[2] !== undefined && args[2] !== null) refuse('failed', 'Widgets send text only, never files.')
      needSending()
      guard(address, text)
      await deps.ensureConversation(address)
      const r = (await inner('thread.post', address, text, { origin: origin() })) as { id: string | null; seq: number | null }
      return { id: r.id, seq: r.seq }
    },
    'thread.answer': async (args) => {
      const address = needAddress('thread.answer', args[0])
      const cardId = args[1]
      if (typeof cardId !== 'string' || !cardId) refuse('failed', 'thread.answer needs a card id')
      // UW50: a choice only, never a secret.
      if (typeof args[2] !== 'string') refuse('failed', 'Widgets answer with a choice only, never a secret.')
      needText('thread.answer', args[2])
      needSending()
      await deps.ensureConversation(address)
      await inner('thread.answer', address, cardId, { answer: args[2], origin: origin() })
      return null
    },
    'compose.draft': (args) => {
      const address = needAddress('compose.draft', args[0])
      inner('compose.draft', address, needText('compose.draft', args[1]))
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
    // K2's own Ask box: the widget supplies no text (GV48 keeps thread:post away).
    'sources.ask': async (args) => {
      const { address, src } = needSource('sources.ask', args[0])
      const key = args[1]
      if (key !== undefined && key !== null && typeof key !== 'string') refuse('failed', 'sources.ask takes an item key (text) or nothing')
      const opened = await src.ask(address, typeof key === 'string' && key ? key.slice(0, 200) : null)
      return { opened }
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
    if (row!.cap !== null && !caps().has(row!.cap)) {
      refuse('cap_not_granted', `${v} needs ${row!.cap}.`, { cap: row!.cap })
    }
    return { verb: v, row: row! }
  }

  function dispose(): void {
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
      if (JSON.stringify(msg).length > ZEN_WIDGET_BUDGET.messageBytes) refuse('too_large', 'A message is over 64 KB.')
      takeToken()
      const args = Array.isArray(m.args) ? (m.args as unknown[]) : []
      const { verb, row } = lookup(m.verb)
      if (typeof m.sub === 'number') {
        const sub = m.sub
        if (row.kind === 'call' && verb !== ZEN_STORE_SUBSCRIBE_VERB) refuse('failed', `${verb} is a call: use k2.call.`)
        if (subs.has(sub)) return null
        const budget = verb !== 'theme.changed'
        const thread = verb === 'thread.subscribe'
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
      const value = await calls[verb](args)
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
