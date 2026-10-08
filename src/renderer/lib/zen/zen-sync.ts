// prd-zen-garden-sync-defaults-v1 S6 — Gardens that sync with K2's defaults,
// and the Garden news the What's new side card shows.
//
// The daemon owns the truth: each Garden's page and theme are either synced
// to K2's defaults or the Garden's own copy (`~/.k2/zen/sync.json`), and the
// daemon decides what counts as news (`news.json`). This module only reads
// those answers and posts gestures. Every request goes to THIS computer's
// daemon (`scopeForHost('local')`, owner token), like the rest of Zen.
//
//   GET  /cli/zen/sync                                  every Garden's state
//   POST /cli/zen/garden/sync {garden, part, sync}      turn sync on / off
//   POST /cli/zen/garden/sync {garden, undo: true}      put the state back
//   POST /cli/zen/garden/sync {garden, keep: "previous", part?}
//   GET  /cli/zen/news                                  unseen Garden news
//   POST /cli/zen/news/seen {ids}                        mark shown news seen
//   GET  /cli/zen/get?garden=<id>&preview=page|theme|both  (zen-api.ts)
//
// A daemon without `zen-sync-v1` answers these routes with "unknown zen
// route": then there are no switches and no card (`supported: false`).
//
// The preview (GS25, GS34) is per window: `useZenSyncPreviewStore` names the
// Garden and part being previewed; `fetchZenPage` adds `preview=` for it.
// Previewing never writes.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { setZenCatalogBadge } from './zen-templates'

function localScope(): ServerScope {
  return scopeForHost('local')
}

export type ZenSyncMode = 'synced' | 'copy'
export type ZenSyncPartName = 'page' | 'theme'
export type ZenSyncParts = ZenSyncPartName | 'both'

export interface ZenSyncPart {
  mode: ZenSyncMode
  /** The archived defaults a copy sits on (`d-…`), null when synced. */
  defaults: string | null
  since: string | null
  reason: string | null
  /** A copy whose synced look would differ (GS30). */
  newerDefault: boolean
  /** The K2 version a copy's defaults came from ("K2 themes from 0.45.1"). */
  k2Version: string | null
}

export interface ZenSyncRow {
  id: string
  name: string
  index: number
  page: ZenSyncPart
  theme: ZenSyncPart
  /** The Garden file's own top-level keys; null when the file can't be read. */
  ownChanges: string[] | null
  undo: { parts: ZenSyncPartName[]; at: string } | null
  keepPrevious: { page: boolean; theme: boolean }
  themeName: string
  themeLabel: string
  themeBuiltin: boolean
  /** The built-in under the Garden's theme ("Basic", "Paper", …). */
  themeBase: { name: string; label: string }
}

export interface ZenSyncList {
  liveDefaults: string
  k2Version: string
  previousDefaults: string | null
  gardens: ZenSyncRow[]
}

export type ZenNewsItem =
  | {
      kind: 'catalog'
      id: string
      short: string
      label: string
      description: string
      template: string
      version: number
    }
  | {
      kind: 'update'
      id: string
      garden: string
      gardenName: string
      part: ZenSyncParts
      previous: string
      keepPrevious: boolean
    }

export interface ZenNews {
  /** Unseen, newest first. */
  items: ZenNewsItem[]
  /** Own-copy Gardens whose K2 default changed since the copy. */
  copiesWithNewerDefault: number
}

export class ZenSyncParseError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ZenSyncParseError'
  }
}

function obj(v: unknown, what: string): Record<string, unknown> {
  if (typeof v !== 'object' || v === null || Array.isArray(v)) throw new ZenSyncParseError(`${what} is not an object`)
  return v as Record<string, unknown>
}

function str(v: unknown, what: string): string {
  if (typeof v !== 'string') throw new ZenSyncParseError(`${what} is not a string`)
  return v
}

function optStr(v: unknown, what: string): string | null {
  if (v === undefined || v === null) return null
  return str(v, what)
}

function bool(v: unknown, what: string): boolean {
  if (typeof v !== 'boolean') throw new ZenSyncParseError(`${what} is not a boolean`)
  return v
}

function num(v: unknown, what: string): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new ZenSyncParseError(`${what} is not a number`)
  return v
}

function arr(v: unknown, what: string): unknown[] {
  if (!Array.isArray(v)) throw new ZenSyncParseError(`${what} is not a list`)
  return v
}

function partName(v: unknown, what: string): ZenSyncPartName {
  if (v === 'page' || v === 'theme') return v
  throw new ZenSyncParseError(`${what} is not page or theme`)
}

function parsePart(v: unknown, what: string): ZenSyncPart {
  const o = obj(v, what)
  const mode = o.mode
  if (mode !== 'synced' && mode !== 'copy') throw new ZenSyncParseError(`${what}.mode is not synced or copy`)
  return {
    mode,
    defaults: optStr(o.defaults, `${what}.defaults`),
    since: optStr(o.since, `${what}.since`),
    reason: optStr(o.reason, `${what}.reason`),
    newerDefault: bool(o.newerDefault, `${what}.newerDefault`),
    k2Version: optStr(o.k2Version, `${what}.k2Version`),
  }
}

/** `GET /cli/zen/sync`, checked at the boundary. */
export function parseZenSyncList(raw: unknown): ZenSyncList {
  const o = obj(raw, 'zen/sync')
  return {
    liveDefaults: str(o.liveDefaults, 'liveDefaults'),
    k2Version: str(o.k2Version, 'k2Version'),
    previousDefaults: optStr(o.previousDefaults, 'previousDefaults'),
    gardens: arr(o.gardens, 'gardens').map((g, i) => {
      const r = obj(g, `gardens[${i}]`)
      const at = `garden ${String(r.id)}`
      const undo = r.undo === null || r.undo === undefined ? null : obj(r.undo, `${at}.undo`)
      const keep = obj(r.keepPrevious, `${at}.keepPrevious`)
      const base = obj(r.themeBase, `${at}.themeBase`)
      return {
        id: str(r.id, `${at}.id`),
        name: str(r.name, `${at}.name`),
        index: num(r.index, `${at}.index`),
        page: parsePart(r.page, `${at}.page`),
        theme: parsePart(r.theme, `${at}.theme`),
        ownChanges:
          r.ownChanges === null || r.ownChanges === undefined
            ? null
            : arr(r.ownChanges, `${at}.ownChanges`).map((k, j) => str(k, `${at}.ownChanges[${j}]`)),
        undo: undo
          ? { parts: arr(undo.parts, `${at}.undo.parts`).map((p, j) => partName(p, `${at}.undo.parts[${j}]`)), at: str(undo.at, `${at}.undo.at`) }
          : null,
        keepPrevious: { page: bool(keep.page, `${at}.keepPrevious.page`), theme: bool(keep.theme, `${at}.keepPrevious.theme`) },
        themeName: str(r.themeName, `${at}.themeName`),
        themeLabel: str(r.themeLabel, `${at}.themeLabel`),
        themeBuiltin: bool(r.themeBuiltin, `${at}.themeBuiltin`),
        themeBase: { name: str(base.name, `${at}.themeBase.name`), label: str(base.label, `${at}.themeBase.label`) },
      }
    }),
  }
}

/** `GET /cli/zen/news`, checked at the boundary. Unknown item kinds (a
 *  newer daemon's) are skipped, not fatal. */
export function parseZenNews(raw: unknown): ZenNews {
  const o = obj(raw, 'zen/news')
  const items: ZenNewsItem[] = []
  for (const [i, it] of arr(o.items, 'items').entries()) {
    const r = obj(it, `items[${i}]`)
    if (r.kind === 'catalog') {
      items.push({
        kind: 'catalog',
        id: str(r.id, `items[${i}].id`),
        short: str(r.short, `items[${i}].short`),
        label: str(r.label, `items[${i}].label`),
        description: str(r.description, `items[${i}].description`),
        template: str(r.template, `items[${i}].template`),
        version: num(r.version, `items[${i}].version`),
      })
    } else if (r.kind === 'update') {
      const part = r.part
      if (part !== 'page' && part !== 'theme' && part !== 'both') throw new ZenSyncParseError(`items[${i}].part is not page, theme or both`)
      items.push({
        kind: 'update',
        id: str(r.id, `items[${i}].id`),
        garden: str(r.garden, `items[${i}].garden`),
        gardenName: str(r.gardenName, `items[${i}].gardenName`),
        part,
        previous: str(r.previous, `items[${i}].previous`),
        keepPrevious: bool(r.keepPrevious, `items[${i}].keepPrevious`),
      })
    }
  }
  return { items, copiesWithNewerDefault: num(o.copiesWithNewerDefault, 'copiesWithNewerDefault') }
}

/** An older daemon (no `zen-sync-v1`) or a computer without Gardens. */
function isUnsupported(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : String(err)
  return /unknown zen route/i.test(msg) || /zen_not_set_up/.test(msg)
}

/** `GET /cli/zen/sync`; null when this daemon has no Garden sync. */
export async function fetchZenSyncList(): Promise<ZenSyncList | null> {
  try {
    return parseZenSyncList(await daemonCliGet<unknown>(localScope(), 'zen/sync'))
  } catch (err) {
    if (isUnsupported(err)) return null
    throw err
  }
}

/** `GET /cli/zen/news`; null when this daemon has no Garden sync. */
export async function fetchZenNews(): Promise<ZenNews | null> {
  try {
    return parseZenNews(await daemonCliGet<unknown>(localScope(), 'zen/news'))
  } catch (err) {
    if (isUnsupported(err)) return null
    throw err
  }
}

export async function setZenGardenSync(garden: string, part: ZenSyncParts, sync: boolean): Promise<void> {
  await daemonCliPost(localScope(), 'zen/garden/sync', { garden, part, sync })
  await loadZenSync()
}

export async function undoZenGardenSync(garden: string): Promise<void> {
  await daemonCliPost(localScope(), 'zen/garden/sync', { garden, undo: true })
  await loadZenSync()
}

export async function keepZenPreviousLook(garden: string, part: ZenSyncParts = 'both'): Promise<void> {
  await daemonCliPost(localScope(), 'zen/garden/sync', { garden, keep: 'previous', part })
  await loadZenSync()
}

/** Mark news ids seen (GS43). An empty list posts nothing. */
export async function markZenNewsSeen(ids: readonly string[]): Promise<void> {
  if (ids.length === 0) return
  await daemonCliPost(localScope(), 'zen/news/seen', { ids })
  const s = useZenSyncStore.getState()
  if (s.news) {
    useZenSyncStore.setState({ news: { ...s.news, items: s.news.items.filter((i) => !ids.includes(i.id)) } })
  }
}

export interface ZenSyncState {
  /** null until the first answer; false = this daemon has no Garden sync. */
  supported: boolean | null
  list: ZenSyncList | null
  news: ZenNews | null
  failure: string | null
}

export const useZenSyncStore = create<ZenSyncState>(() => ({ supported: null, list: null, news: null, failure: null }))

let syncSeq = 0

/** Load `zen/sync` and `zen/news` into the store. A newer call wins. */
export async function loadZenSync(): Promise<void> {
  const seq = ++syncSeq
  try {
    const [list, news] = await Promise.all([fetchZenSyncList(), fetchZenNews()])
    if (seq !== syncSeq) return
    useZenSyncStore.setState({ supported: list !== null, list, news, failure: null })
  } catch (err) {
    if (seq !== syncSeq) return
    useZenSyncStore.setState({ failure: err instanceof Error ? err.message : String(err) })
  }
}

/** Only `zen/news` (the What's new card). */
export async function loadZenNews(): Promise<ZenNews | null> {
  const news = await fetchZenNews()
  useZenSyncStore.setState((s) => ({ news, supported: s.supported ?? (news === null ? false : null) }))
  return news
}

// ── preview (GS25, GS34) ──────────────────────────────────────────────

export interface ZenSyncPreview {
  gardenId: string
  part: ZenSyncParts
}

export const useZenSyncPreviewStore = create<{
  preview: ZenSyncPreview | null
  start(gardenId: string, part: ZenSyncParts): void
  end(): void
}>((set) => ({
  preview: null,
  start(gardenId, part) {
    set({ preview: { gardenId, part } })
  },
  end() {
    set({ preview: null })
  },
}))

/** The `preview=` value `GET /cli/zen/get` takes for this Garden, if any. */
export function zenSyncPreviewParam(gardenId: string): ZenSyncParts | undefined {
  const p = useZenSyncPreviewStore.getState().preview
  return p && p.gardenId === gardenId ? p.part : undefined
}

// ── the What's new side card (GS40–GS43) ──────────────────────────────

/** Below this width the card is hidden (and nothing is marked seen). */
export const ZEN_NEWS_CARD_MIN_WIDTH = 1000

export interface ZenNewsCardModel {
  /** The one item drawn, newest first. */
  item: ZenNewsItem
  /** How many more unseen items Settings → Gardens lists. */
  more: number
  /** "Your own copies didn't change. Sync them in Settings → Gardens." */
  copiesLine: boolean
  /** What closing the dialog marks seen: the item the card showed. */
  shownIds: string[]
}

/** Whether and what the card shows (GS42): the dialog is visible, this
 *  computer's daemon has unseen news, and the window is wide enough. */
export function zenNewsCardModel(
  news: ZenNews | null,
  opts: { dialogVisible: boolean; viewportWidth: number },
): ZenNewsCardModel | null {
  if (!opts.dialogVisible || opts.viewportWidth < ZEN_NEWS_CARD_MIN_WIDTH) return null
  if (!news || news.items.length === 0) return null
  const item = news.items[0]
  return {
    item,
    more: news.items.length - 1,
    copiesLine: news.copiesWithNewerDefault > 0,
    shownIds: [item.id],
  }
}

/** The card's text for one item (GS41). */
export function zenNewsCardText(item: ZenNewsItem): { title: string; lead: string | null; body: string; button: string | null } {
  if (item.kind === 'catalog') {
    return { title: 'New in the Garden catalog', lead: `${item.label}.`, body: item.description, button: 'See it in New Garden' }
  }
  return {
    title: `${item.gardenName} has a fresh look`,
    lead: null,
    body: 'It syncs with K2’s default, so it got this release’s improvements.',
    button: item.keepPrevious ? 'Keep my previous look' : null,
  }
}

/** Catalog shorts with unseen news (the New Garden "New" badge, GS43b). */
export function zenNewCatalogShorts(news: ZenNews | null): string[] {
  return news ? news.items.flatMap((i) => (i.kind === 'catalog' ? [i.short] : [])) : []
}

/** Fired when the What's new card's "See it in New Garden" asks Zen's
 *  New Garden to open with this catalog entry highlighted
 *  (`detail: {short}`). The Garden switcher (band pill or menu section)
 *  listens (`useZenOpenNewGardenRequest`, ZenNewGarden.tsx). */
export const ZEN_OPEN_NEW_GARDEN_EVENT = 'k2:zen-open-new-garden'

/** The catalog's "New" badges follow the loaded news: B4's catalog draws
 *  whatever `setZenCatalogBadge` sets, and this module is the one that
 *  knows what's new. Badges this module set and the news no longer lists
 *  (seen, or a newer load) are cleared; other badges are left alone. */
let badged: string[] = []
export function syncZenNewsBadges(news: ZenNews | null): void {
  const next = zenNewCatalogShorts(news)
  for (const short of badged) if (!next.includes(short)) setZenCatalogBadge(short, null)
  for (const short of next) setZenCatalogBadge(short, 'New')
  badged = next
}
useZenSyncStore.subscribe((s, prev) => {
  if (s.news !== prev.news) syncZenNewsBadges(s.news)
})

/** Tests only. */
export function __resetZenSyncForTests(): void {
  syncSeq = 0
  useZenSyncStore.setState({ supported: null, list: null, news: null, failure: null })
  badged = []
  useZenSyncPreviewStore.setState({ preview: null })
}
