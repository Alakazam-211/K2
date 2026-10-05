// prd-zen-mode-v1 Z33–Z36 and prd-zen-gardens-v1 G29 — the one bridge every
// Zen widget talks through.
//
// A widget never receives a token, a scope, or a `daemonCli*` function. It
// gets a `ZenWidgetBridge`: every call names a VERB, every verb needs a CAP,
// and a widget's caps are what its source declared (built-ins: the template
// TOML, granted by K2, source `builtin`; v2 user widgets: manifest +
// grants.json). An undeclared verb throws `cap_not_granted`, loudly.
//
// Verb table (G29):
//   - no cap, every page has them: `gardens.list`, `gardens.current`,
//     `gardens.switch` (every page must have a Garden switcher), `zen.exit`,
//     `controls.bind`, `theme.get`;
//   - `gardens:manage` (granted by K2 to the template's own controls, never
//     to a widget in v1): `gardens.create`, `gardens.rename`,
//     `gardens.delete`;
//   - `agents:read`: `homes.list` (Home names, for the Agents widget's Home
//     picker), `agents.home` / `agents.setHome` (that picker's view state),
//     `agents.local` (the agents on this computer, for Ask my agent), and
//     the v1 data verbs `agents.list`, `agents.subscribe`, `conversation.*`;
//   - `thread:post`: `compose.draft(address, text)` sets a conversation's
//     message box, never sends.
// `homes.select` is gone: Zen never changes the window's Home.
//
// The data verbs are provided by `zen-data.ts` (and `agents.add` by
// `zen-add-agent.ts`) through `registerZenVerb`; until one is registered
// the verb throws `verb_unavailable`. A data verb gets the calling widget's
// id plus the page and Garden it sits on (`ZenVerbCtx`).
//
// Which server a data verb talks to (Z35) and the per-row role check (Z36)
// belong to those implementations: a row on the window's server uses the
// primary room, any other row its pinned room (`scopeForHost(hostKey)`),
// and Zen never switches the window's server.

import type { ZenControlRegistry, ZenBindKind } from './zen-controls'
import type { ZenResolvedPage } from './zen-page'

export const ZEN_VERBS = {
  'agents.list': 'agents:read',
  'agents.subscribe': 'agents:read',
  'agents.add': 'agents:add',
  'agents.home': 'agents:read',
  'agents.setHome': 'agents:read',
  'agents.local': 'agents:read',
  'presence.get': 'presence:read',
  'presence.subscribe': 'presence:read',
  'conversation.open': 'agents:read',
  'conversation.close': 'agents:read',
  'thread.read': 'thread:read',
  'thread.subscribe': 'thread:read',
  'thread.markRead': 'thread:read',
  'thread.post': 'thread:post',
  'thread.answer': 'thread:post',
  'thread.void': 'thread:post',
  'compose.draft': 'thread:post',
  'homes.list': 'agents:read',
  'gardens.list': null,
  'gardens.current': null,
  'gardens.switch': null,
  'gardens.create': 'gardens:manage',
  'gardens.rename': 'gardens:manage',
  'gardens.delete': 'gardens:manage',
  'zen.exit': null,
  'controls.bind': null,
  'theme.get': null,
} as const satisfies Record<string, string | null>

export type ZenVerb = keyof typeof ZEN_VERBS
export type ZenCap = Exclude<(typeof ZEN_VERBS)[ZenVerb], null>

export type ZenBridgeErrorCode = 'cap_not_granted' | 'verb_unavailable' | 'unknown_verb'

export class ZenBridgeError extends Error {
  readonly code: ZenBridgeErrorCode
  readonly verb: string
  constructor(code: ZenBridgeErrorCode, verb: string, detail?: string) {
    super(`${code}: ${verb}${detail ? ` (${detail})` : ''}`)
    this.name = 'ZenBridgeError'
    this.code = code
    this.verb = verb
  }
}

/** Who is calling a data verb: the widget, and the page and Garden it is on. */
export interface ZenVerbCtx {
  widgetId: string
  /** The page on screen (safe mode: the built-in page). */
  page(): ZenResolvedPage
  /** The Garden on screen ('' before the list is read). */
  gardenId(): string
}

/** A data verb's implementation. */
export type ZenVerbImpl = (ctx: ZenVerbCtx, ...args: unknown[]) => unknown

const verbImpls = new Map<ZenVerb, ZenVerbImpl>()

/** Plug-in point: provide a data verb. Returns the unregister. */
export function registerZenVerb(verb: ZenVerb, impl: ZenVerbImpl): () => void {
  if (!Object.prototype.hasOwnProperty.call(ZEN_VERBS, verb)) throw new Error(`zen bridge: unknown verb ${verb}`)
  if (ZEN_VERBS[verb] === null || BUILTIN_VERBS.has(verb)) throw new Error(`zen bridge: ${verb} is built in`)
  verbImpls.set(verb, impl)
  return () => {
    if (verbImpls.get(verb) === impl) verbImpls.delete(verb)
  }
}

export interface ZenHomeSummary {
  id: string
  name: string
}

export interface ZenGardenSummary {
  id: string
  name: string
  /** 1-based position (⌥⌘N for 1–9). */
  index: number
}

/** What the page runtime gives the bridge. */
export interface ZenBridgeHost {
  gardens(): ZenGardenSummary[]
  /** The Garden on screen ('' before the list is read). */
  currentGardenId(): string
  switchGarden(id: string): void
  createGarden(name: string): Promise<ZenGardenSummary>
  renameGarden(id: string, name: string): Promise<void>
  deleteGarden(id: string): Promise<void>
  /** Every Home, in `k2.homes.v1` order. */
  homes(): ZenHomeSummary[]
  exit(): void
  controls: ZenControlRegistry
  page(): ZenResolvedPage
}

/** What a widget (or the page's own controls) holds. */
export interface ZenWidgetBridge {
  readonly widgetId: string
  readonly caps: ReadonlySet<string>
  /** Call a verb by name; the cap check runs first. */
  call(verb: ZenVerb, ...args: unknown[]): unknown
  gardens: {
    list(): ZenGardenSummary[]
    current(): ZenGardenSummary | null
    switch(id: string): void
    create(name: string): Promise<ZenGardenSummary>
  }
  homes: { list(): ZenHomeSummary[] }
  zen: { exit(): void }
  controls: { bind(kind: ZenBindKind, el: HTMLElement, gardenId?: string): () => void }
  theme: { get(): { theme: unknown; chrome: unknown; motion: unknown } }
}

const BUILTIN_VERBS = new Set<ZenVerb>([
  'homes.list',
  'gardens.list',
  'gardens.current',
  'gardens.switch',
  'gardens.create',
  'gardens.rename',
  'gardens.delete',
  'zen.exit',
  'controls.bind',
  'theme.get',
])

function needString(verb: ZenVerb, v: unknown, what: string): string {
  if (typeof v !== 'string' || !v.trim()) throw new ZenBridgeError('unknown_verb', verb, `needs ${what}`)
  return v
}

/** Build the bridge for one widget with its declared caps. */
export function createZenBridge(host: ZenBridgeHost, widget: { id: string; caps: readonly string[] }): ZenWidgetBridge {
  const caps = new Set(widget.caps)
  const current = (): ZenGardenSummary | null => {
    const id = host.currentGardenId()
    return host.gardens().find((g) => g.id === id) ?? null
  }
  const builtins: Partial<Record<ZenVerb, (...args: unknown[]) => unknown>> = {
    'homes.list': () => host.homes(),
    'gardens.list': () => host.gardens(),
    'gardens.current': () => current(),
    'gardens.switch': (id) => host.switchGarden(needString('gardens.switch', id, 'a Garden id')),
    'gardens.create': (name) => host.createGarden(needString('gardens.create', name, 'a name')),
    'gardens.rename': (id, name) =>
      host.renameGarden(needString('gardens.rename', id, 'a Garden id'), needString('gardens.rename', name, 'a name')),
    'gardens.delete': (id) => host.deleteGarden(needString('gardens.delete', id, 'a Garden id')),
    'zen.exit': () => host.exit(),
    'controls.bind': (kind, el, gardenId) =>
      host.controls.bind(kind as ZenBindKind, el as HTMLElement, gardenId as string | undefined),
    'theme.get': () => {
      const p = host.page()
      return { theme: p.theme, chrome: p.chrome, motion: p.motion }
    },
  }
  const ctx: ZenVerbCtx = {
    widgetId: widget.id,
    page: () => host.page(),
    gardenId: () => host.currentGardenId(),
  }
  const call = (verb: ZenVerb, ...args: unknown[]): unknown => {
    if (!Object.prototype.hasOwnProperty.call(ZEN_VERBS, verb)) {
      throw new ZenBridgeError('unknown_verb', String(verb))
    }
    const cap = ZEN_VERBS[verb]
    if (cap !== null && !caps.has(cap)) throw new ZenBridgeError('cap_not_granted', verb, `needs ${cap}`)
    const builtin = builtins[verb]
    if (builtin) return builtin(...args)
    const impl = verbImpls.get(verb)
    if (!impl) throw new ZenBridgeError('verb_unavailable', verb)
    return impl(ctx, ...args)
  }
  return {
    widgetId: widget.id,
    caps,
    call,
    gardens: {
      list: () => call('gardens.list') as ZenGardenSummary[],
      current: () => call('gardens.current') as ZenGardenSummary | null,
      switch: (id) => void call('gardens.switch', id),
      create: (name) => call('gardens.create', name) as Promise<ZenGardenSummary>,
    },
    homes: { list: () => call('homes.list') as ZenHomeSummary[] },
    zen: { exit: () => void call('zen.exit') },
    controls: { bind: (kind, el, gardenId) => call('controls.bind', kind, el, gardenId) as () => void },
    theme: { get: () => call('theme.get') as { theme: unknown; chrome: unknown; motion: unknown } },
  }
}

/** Tests only. */
export function __resetZenVerbsForTests(): void {
  verbImpls.clear()
}
