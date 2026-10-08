// prd-zen-mode-v1 Z33–Z36 and prd-zen-gardens-v1 G29 — the one bridge every
// Zen widget talks through.
//
// A widget never receives a token, a scope, or a `daemonCli*` function. It
// gets a `ZenWidgetBridge`: every call names a VERB, every verb needs a CAP,
// and a widget's caps are what its source declared (built-ins: the template
// TOML, granted by K2, source `builtin`; v2 user widgets: their manifest
// caps, no grant). An undeclared verb throws `cap_not_granted`, loudly.
//
// Verb table (G29):
//   - no cap, every page has them: `gardens.list`, `gardens.current`,
//     `gardens.switch` (every page must have a Garden switcher), `zen.exit`,
//     `controls.bind`, `theme.get`;
//   - `gardens:manage` (granted by K2 to the template's own controls, never
//     to a widget in v1): `gardens.create(name, template?, {ask})`,
//     `gardens.rename`, `gardens.delete`;
//   - `gardens:template` (the built-in `garden-empty` widget only):
//     `gardens.empty()` says whether the Garden on screen is the empty page,
//     and `gardens.useTemplate(template, {force})` turns THAT Garden (never
//     another) into a built-in template: "Start with the default";
//   - `agents:read`: `homes.list` (Home names, for the Agents widget's Home
//     picker), `agents.home` / `agents.setHome` (that picker's view state),
//     `agents.local` (the agents on this computer, for Ask my agent), and
//     the v1 data verbs `agents.list`, `agents.subscribe`, `conversation.*`;
//   - `thread:post`: `compose.draft(address, text)` sets a conversation's
//     message box, never sends;
//   - `agents:read` also: `focusGroups.get` / `focusGroups.set` /
//     `focusGroups.subscribe` (the Agents view's focus-group dropdown);
//   - `app:navigate` (the `nav-rail` widget): `app.open(page)` switches the
//     Garden's view in this window (My Home, Agents, Projects, Tickets; Zen
//     stays on), `app.current()` / `app.subscribeCurrent(fn)` read it, and
//     `app.badges()` / `app.subscribe(fn)` read the top bar's Tickets badge.
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
import { BLANK_TEMPLATE_ID } from './zen-page'
import { ZEN_VERBS } from './zen-verbs.generated'
import type { CatalogErrorCode } from '../contract/catalog-types'
import { isZenTemplateShort } from './zen-templates'

// UWA4: generated from the verb catalog (`contract-gen`), with the custom
// widgets' own allowlist beside it. A pinned test keeps it today's 37.
export { ZEN_VERBS }

export type ZenVerb = keyof typeof ZEN_VERBS
export type ZenCap = Exclude<(typeof ZEN_VERBS)[ZenVerb], null>

/** A subset of the shared error list (UWA10, `CatalogErrorCode`). */
export type ZenBridgeErrorCode = Extract<CatalogErrorCode, 'cap_not_granted' | 'verb_unavailable' | 'unknown_verb'>

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

/** What a Garden starts as, or turns into (Rosson 2026-10-04): `texting`
 *  is Garden 1's page ("the default"), `blank` the empty page an agent
 *  builds; the Garden catalog adds more by short name (`diary`,
 *  prd-zen-user-widgets-v2 UWB23, R5), read from `GET /cli/zen/templates`.
 *  The daemon refuses a name it doesn't know. */
export type ZenGardenTemplate = 'texting' | 'blank' | (string & {})

/** `gardens.create` options: `ask` opens Ask my agent on the new (empty)
 *  Garden once it shows. */
export interface ZenGardenCreateOptions {
  ask?: boolean
}

/** What `gardens.useTemplate` did: `changed` is false when the Garden
 *  already was that template with nothing of its own. */
export interface ZenGardenTemplateResult {
  changed: boolean
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
  createGarden(name: string, template?: ZenGardenTemplate, opts?: ZenGardenCreateOptions): Promise<ZenGardenSummary>
  /** Turn Garden `id` into a built-in template (`garden/template`). */
  useGardenTemplate(id: string, template: ZenGardenTemplate, force: boolean): Promise<ZenGardenTemplateResult>
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
    create(name: string, template?: ZenGardenTemplate, opts?: ZenGardenCreateOptions): Promise<ZenGardenSummary>
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
  'gardens.empty',
  'gardens.useTemplate',
  'zen.exit',
  'controls.bind',
  'theme.get',
])

/** A template's short name: `texting`, `blank`, or a catalog Garden's that
 *  this computer's daemon lists (UWB23, `zen-templates.ts`). */
function needTemplate(verb: ZenVerb, v: unknown, optional: boolean): ZenGardenTemplate | undefined {
  if (v === undefined && optional) return undefined
  if (!isZenTemplateShort(v)) {
    throw new ZenBridgeError('unknown_verb', verb, 'needs a template: texting, blank or a ready-made Garden')
  }
  return v
}

/** Whether a resolved page is the empty Garden: the blank template showing
 *  only its own empty-Garden widget (no widgets of the Garden's own). */
export function isEmptyZenGardenPage(p: ZenResolvedPage): boolean {
  return p.template === BLANK_TEMPLATE_ID && p.widgets.length === 1 && p.widgets[0].kind === 'garden-empty'
}

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
    'gardens.create': (name, template, opts) =>
      host.createGarden(
        needString('gardens.create', name, 'a name'),
        needTemplate('gardens.create', template, true),
        (opts ?? {}) as ZenGardenCreateOptions,
      ),
    'gardens.rename': (id, name) =>
      host.renameGarden(needString('gardens.rename', id, 'a Garden id'), needString('gardens.rename', name, 'a name')),
    'gardens.delete': (id) => host.deleteGarden(needString('gardens.delete', id, 'a Garden id')),
    'gardens.empty': () => isEmptyZenGardenPage(host.page()),
    'gardens.useTemplate': (template, opts) => {
      const id = host.currentGardenId()
      if (!id) throw new ZenBridgeError('unknown_verb', 'gardens.useTemplate', 'no Garden on screen')
      const force = (opts as { force?: unknown } | undefined)?.force === true
      return host.useGardenTemplate(id, needTemplate('gardens.useTemplate', template, false) as ZenGardenTemplate, force)
    },
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
      create: (name, template, opts) => call('gardens.create', name, template, opts) as Promise<ZenGardenSummary>,
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
