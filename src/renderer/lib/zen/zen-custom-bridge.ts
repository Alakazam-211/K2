// prd-zen-user-widgets-v2 UW15–UW19, UW26, UW29, UWA4, UWA6, UWA10,
// UWB7–UWB11 — the custom layer: what a sealed widget's frame may ask K2
// for, checked on every message before anything reaches K2's own verbs.
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
//   4. the cap: requested ∩ granted ∩ the four widget caps, as the daemon
//      sent it and the renderer intersected again (UW26) → `cap_not_granted`;
//   5. the binding: an address must be one of the widget's bound rows (its
//      granted scope now, UWB7) → `not_bound`. Thread verbs take any bound
//      address (UWB10);
//   6. sending: `thread.post` / `thread.answer` need Sending on (UWB9) →
//      `sending_off`; text only, ≤ 4,000 characters, never files or secrets
//      (UW19, UW50); the runaway guard (120 posts or 20 identical texts to
//      one agent in 10 minutes, R6) turns Sending off on the grant and
//      unloads the frame;
//   7. limits: 16 live subscriptions, 16 Thread subscriptions, at most 8
//      live servers (`zen-custom-scope`), `gardens.switch` once every 2 s
//      and never in the first 2 s after mount.
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
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

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

export class ZenCustomRefusal extends Error {
  readonly wire: CatalogWireError
  constructor(code: CatalogErrorCode, message: string, extra: Partial<Pick<CatalogWireError, 'cap' | 'room' | 'feature'>> = {}) {
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
  /** The widget's inner bridge, built with its effective caps. */
  inner: ZenWidgetBridge
  /** The bound rows now (the grant's scope, never a fallback). */
  boundRows(): ZenAgentRow[]
  /** UWB10: resolve a bound agent's conversation without picking it. */
  ensureConversation(address: string): Promise<void>
  /** Push a subscription value to the frame. */
  push(sub: number, value: unknown): void
  /** Stop this widget (its frame goes; K2 draws the card). */
  stop(reason: ZenWidgetStopReason): void
  /** Runaway: turn Sending off on the grant (sets the daemon's pause). */
  sendingOffForRunaway(): Promise<void>
  /** The Garden's theme now (`ThemeInfo`), and its changes (UW39). */
  theme(): unknown
  onThemeChange(cb: () => void): () => void
  now(): number
}

export interface ZenCustomLayer {
  /** One message from the frame's port; the reply to post, if any. */
  handle(msg: unknown): Promise<ZenFrameReply | null>
  /** Live subscriptions (tests). */
  readonly subscriptions: number
  dispose(): void
}

type Row = { cap: UserWidgetCap | null; kind: 'call' | 'subscribe' | 'event' }

const CUSTOM: Record<string, Row> = ZEN_CUSTOM_VERBS as unknown as Record<string, Row>
const WIDGET_CAPS: ReadonlySet<string> = new Set(USER_WIDGET_CAPS)

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
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

  const caps = (): ReadonlySet<string> => {
    const w = deps.widget()
    // UW26 step 2: intersect again; `source: user` whatever the body said.
    return new Set(w.caps.filter((c) => WIDGET_CAPS.has(c) && w.requested.includes(c)))
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

  const needText = (verb: string, v: unknown): string => {
    if (typeof v !== 'string') refuse('failed', `${verb} needs text`)
    const text = v as string
    if ([...text].length > ZEN_WIDGET_BUDGET.postChars) refuse('too_large', `${verb} takes up to 4,000 characters`)
    return text
  }

  const needSending = (): void => {
    const g = deps.widget().grant
    if (!g || !g.sending || g.paused) refuse('sending_off', 'Sending is turned off for this widget.')
  }

  /** R6: record a post; trip the guard on the 121st, or the 21st identical text to one agent. */
  const guard = (address: string, text: string): void => {
    const now = deps.now()
    while (posts.length > 0 && now - posts[0].at > ZEN_RUNAWAY.windowMs) posts.shift()
    const key = `${address}\n${text.trim()}`
    const same = posts.filter((p) => p.key === key).length
    if (posts.length + 1 > ZEN_RUNAWAY.posts || same + 1 > ZEN_RUNAWAY.identical) {
      void deps.sendingOffForRunaway().catch((err: unknown) => console.warn('[zen] runaway: turning sending off failed:', err))
      stop('runaway')
      throw new ZenCustomRefusal('sending_off', 'This widget sent too many messages, so K2 turned its sending off.')
    }
    posts.push({ at: now, key })
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
      const origin = { widget: deps.widget().widget, garden: deps.gardenId() }
      const r = (await inner('thread.post', address, text, { origin })) as { id: string | null; seq: number | null }
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
      await inner('thread.answer', address, cardId, args[2])
      return null
    },
    'compose.draft': (args) => {
      const address = needAddress('compose.draft', args[0])
      inner('compose.draft', address, needText('compose.draft', args[1]))
      return null
    },
  }

  // ── subscriptions ────────────────────────────────────────────────────────

  const subscribers: Record<string, (args: unknown[], push: (v: unknown) => void) => () => void> = {
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
        if (row.kind === 'call') refuse('failed', `${verb} is a call: use k2.call.`)
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
        try {
          entry.off = subscribers[verb](args, (value) => {
            if (!stopped && subs.get(sub) === entry) deps.push(sub, zenPlain(value))
          })
        } catch (err) {
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
      if (id === null) return null
      return { id, ok: false, error: wire }
    }
  }

  return {
    handle,
    get subscriptions() {
      return subs.size
    },
    dispose,
  }
}

/** Any thrown value as the shared error list (UWA10). */
export function toRefusal(err: unknown): ZenCustomRefusal {
  if (err instanceof ZenCustomRefusal) return err
  if (err instanceof ZenBridgeError) {
    if (err.code === 'cap_not_granted') return new ZenCustomRefusal('cap_not_granted', err.message)
    if (err.code === 'verb_unavailable') return new ZenCustomRefusal('verb_unavailable', err.message)
    return new ZenCustomRefusal('failed', err.message)
  }
  return new ZenCustomRefusal('failed', err instanceof Error ? err.message : String(err))
}
