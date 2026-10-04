// prd-zen-mode-v1 Z33–Z36 — the one bridge every Zen widget talks through.
//
// A widget never receives a token, a scope, or a `daemonCli*` function. It
// gets a `ZenWidgetBridge`: every call names a VERB, every verb needs a CAP,
// and a widget's caps are what its source declared (built-ins: the template
// TOML, granted by K2, source `builtin`; v2 user widgets: manifest +
// grants.json). An undeclared verb throws `cap_not_granted`, loudly.
//
// v1 verb table (Z34). The no-cap verbs every page must have are built here
// (homes.list / homes.select, zen.exit, controls.bind, theme.get). The
// data verbs (agents.*, presence.*, conversation.*, thread.*) are provided by
// the S6 widgets slice through `registerZenVerb`; until one is registered
// the verb throws `verb_unavailable`.
//
// `agents.add` (cap `agents:add`) opens K2's own Add agent picker for the
// current Home (the regular Home's searchable picker): the human picks, K2
// writes the row. A widget never writes Home rows itself. The template's
// controls get this cap from K2 (the bottom-left Add agent button).
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
  'homes.list': null,
  'homes.select': null,
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

/** A data verb's implementation (S6). `ctx` names the calling widget. */
export type ZenVerbImpl = (ctx: { widgetId: string }, ...args: unknown[]) => unknown

const verbImpls = new Map<ZenVerb, ZenVerbImpl>()

/** S6 plug-in point: provide a data verb. Returns the unregister. */
export function registerZenVerb(verb: ZenVerb, impl: ZenVerbImpl): () => void {
  if (ZEN_VERBS[verb] === null) throw new Error(`zen bridge: ${verb} is built in`)
  verbImpls.set(verb, impl)
  return () => {
    if (verbImpls.get(verb) === impl) verbImpls.delete(verb)
  }
}

export interface ZenHomeSummary {
  id: string
  name: string
}

/** What the page runtime gives the bridge. */
export interface ZenBridgeHost {
  homes(): ZenHomeSummary[]
  selectedHomeId(): string
  selectHome(id: string): void
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
  homes: { list(): ZenHomeSummary[]; selected(): string; select(id: string): void }
  zen: { exit(): void }
  controls: { bind(kind: ZenBindKind, el: HTMLElement, homeId?: string): () => void }
  theme: { get(): { theme: unknown; chrome: unknown; motion: unknown } }
}

/** Build the bridge for one widget with its declared caps. */
export function createZenBridge(host: ZenBridgeHost, widget: { id: string; caps: readonly string[] }): ZenWidgetBridge {
  const caps = new Set(widget.caps)
  const builtins: Partial<Record<ZenVerb, (...args: unknown[]) => unknown>> = {
    'homes.list': () => host.homes(),
    'homes.select': (id) => {
      if (typeof id !== 'string') throw new ZenBridgeError('unknown_verb', 'homes.select', 'needs a Home id')
      host.selectHome(id)
    },
    'zen.exit': () => host.exit(),
    'controls.bind': (kind, el, homeId) =>
      host.controls.bind(kind as ZenBindKind, el as HTMLElement, homeId as string | undefined),
    'theme.get': () => {
      const p = host.page()
      return { theme: p.theme, chrome: p.chrome, motion: p.motion }
    },
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
    return impl({ widgetId: widget.id }, ...args)
  }
  return {
    widgetId: widget.id,
    caps,
    call,
    homes: {
      list: () => call('homes.list') as ZenHomeSummary[],
      selected: () => host.selectedHomeId(),
      select: (id) => void call('homes.select', id),
    },
    zen: { exit: () => void call('zen.exit') },
    controls: { bind: (kind, el, homeId) => call('controls.bind', kind, el, homeId) as () => void },
    theme: { get: () => call('theme.get') as { theme: unknown; chrome: unknown; motion: unknown } },
  }
}

/** Tests only. */
export function __resetZenVerbsForTests(): void {
  verbImpls.clear()
}
