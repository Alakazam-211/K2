// prd-zen-mode-v1 Z9, Z10, Z15 and prd-zen-gardens-v1 G10–G12, G24, G38 —
// the resolved page of one Garden, as the renderer reads it from
// `GET /cli/zen/get?garden=<id>` on THIS computer's daemon.
//
// The daemon resolves the Garden's template (`k2.texting@1` or
// `k2.blank@1`), the Garden file's layout and built-in widgets (G38), and
// the theme stack, and returns the structure; the renderer draws from it.
// Every body is parsed here, at the boundary: nothing past `parseZenGet`
// sees raw JSON. The inner shapes of `layout`, `widgets` and `controls` are
// this client's reading of the contract; see `docs/zen-contract.md`.
//
// prd-zen-freeform-chrome (S2): K2's controls are widgets too. Where they
// sit is `placement` (the answer's `page.chrome`, `page.bands`,
// `page.edges` and `page.menus`, FC29). A daemon without `zen-chrome-v1`
// sends none of those: the renderer then builds the template's own chrome
// from its built-in page (FC32), so the page looks exactly like before.
//
// The built-in `k2.texting@1` page is also the safe-mode page (Z29): safe
// mode never reads the user's files, so it renders `BUILTIN_TEXTING_PAGE`,
// with the template's chrome (FC22).

import type { ZenCustomWidgetPayload } from './zen-custom-types'
import { parseZenCustomWidget } from './zen-custom-payload'

/** The two controls every Garden page must draw and K2 checks (G24,
 *  Rosson 2026-10-04): the Zen toggle (the way out) and the Garden
 *  switcher. The drag region is still drawn and bound (it moves the
 *  window) but is not checked, so it can never put a window in safe mode. */
export const ZEN_REQUIRED_CONTROLS = ['zen-toggle', 'garden-switcher'] as const
export type ZenControlKind = (typeof ZEN_REQUIRED_CONTROLS)[number]

/** One validation finding, with where it is (Z13). */
export interface ZenIssue {
  file: string
  line: number
  col: number
  message: string
}

/** v1 layout: columns, left to right. */
export interface ZenLayout {
  kind: 'columns'
  /** Percent widths, one per column. */
  split: number[]
  /** Minimum px widths, one per column (0 = none). */
  minWidths: number[]
}

/** Where a widget sits (Rosson 2026-10-06; prd-zen-freeform-chrome FC7):
 *  a layout column (the default), or a band of the page (`top`,
 *  `bottom`). The daemon's schema owns the values and which kinds fit a
 *  band (`BAND_WIDGET_KINDS`). A string, so later slots are new values. */
export type ZenWidgetSlot = string
/** The default slot: a layout column. */
export const ZEN_COLUMN_SLOT = 'column'
/** Band slots this renderer draws (`ZenBands`): the full-width rows above
 *  and below the columns. */
export const ZEN_BAND_SLOTS: readonly string[] = ['top', 'bottom']
/** The slot of a Zen control inside a `menu` widget (FC7). */
export const ZEN_MENU_SLOT = 'menu'

/** The slot a widget sits in: a band this renderer knows, else a column
 *  (a slot from a newer daemon falls back to its column). */
export function zenWidgetSlot(w: Pick<ZenWidgetDecl, 'slot'>): ZenWidgetSlot {
  return w.slot && ZEN_BAND_SLOTS.includes(w.slot) ? w.slot : ZEN_COLUMN_SLOT
}

/** Does the widget sit in a band (not a column)? */
export function zenWidgetInBand(w: Pick<ZenWidgetDecl, 'slot'>): boolean {
  return zenWidgetSlot(w) !== ZEN_COLUMN_SLOT
}

/** `align` (FC8): where an item sits in a band or a column edge. */
export type ZenAlign = 'start' | 'center' | 'end'
export const ZEN_ALIGNS: readonly ZenAlign[] = ['start', 'center', 'end']
/** `edge` (FC8): which edge of its column a row item sits at. */
export type ZenEdge = 'top' | 'bottom'
export const ZEN_EDGES: readonly ZenEdge[] = ['top', 'bottom']

/** Chrome kinds (FC1): K2's own controls, placed like widgets. */
export const ZEN_CHROME_KINDS = ['garden-switcher', 'zen-toggle', 'usage', 'theme-picker', 'menu'] as const
export type ZenChromeKind = (typeof ZEN_CHROME_KINDS)[number]
/** What a `menu` may hold (FC9; the daemon's `MENU_ITEM_KINDS`). */
export const ZEN_MENU_ITEM_KINDS: readonly string[] = ['zen-toggle', 'garden-switcher', 'theme-picker', 'usage']

/** One chrome item as the daemon sends it (`page.chrome.items`, FC29). */
export interface ZenChromeItem {
  id: string
  kind: string
  /** `top` | `bottom` (a band), `column` (a column edge) or `menu`. */
  slot: string
  /** `slot = "column"`: the column it sits in. */
  column?: number
  /** `slot = "column"`: that column's top or bottom edge. */
  edge?: ZenEdge
  /** A band or edge item: start, center or end. */
  align?: ZenAlign
  /** `slot = "menu"`: the menu widget that holds it. */
  menu?: string
  props: Record<string, unknown>
  caps: string[]
}

/** One band's (or column edge's) ids in draw order, by alignment. */
export interface ZenRowGroups {
  start: string[]
  center: string[]
  end: string[]
}

/** One column edge that holds something (`page.edges`). */
export interface ZenColumnEdge extends ZenRowGroups {
  column: number
  edge: ZenEdge
}

/**
 * Where K2's controls (and band and edge content) sit on the page
 * (prd-zen-freeform-chrome FC29): `page.chrome`, `page.bands`,
 * `page.edges` and `page.menus`, in draw order. A daemon without
 * `zen-chrome-v1` sends none: then this is the template's own chrome
 * (FC32), built here from the built-in page.
 */
export interface ZenPlacement {
  /** `garden`: the Garden file placed its own; `template`: the template's. */
  from: 'template' | 'garden'
  items: ZenChromeItem[]
  bands: { top: ZenRowGroups | null; bottom: ZenRowGroups | null }
  edges: ZenColumnEdge[]
  /** Menu id → the ids it holds, in file order. */
  menus: Record<string, string[]>
}

/** One widget placed on the page. Built-ins in v1; the same shape carries
 *  v2 user widgets. `caps` are what its source declared (Z33). */
export interface ZenWidgetDecl {
  id: string
  /** Which widget renders it: `agents`, `conversation` (v1 built-ins). */
  kind: string
  /** Column index in `layout` (ignored for a band widget). */
  column: number
  /** A band slot (`top`, `bottom`); absent means a column. */
  slot?: ZenWidgetSlot
  /** A row item at a column edge (only when the file set it, FC28). */
  edge?: ZenEdge
  /** Where in its band or edge (only when the file set it, FC28). */
  align?: ZenAlign
  props: Record<string, unknown>
  caps: string[]
  /** `builtin` in v1 (granted by K2); `user` for a custom widget. */
  source: string
  /** `kind: "custom"` only (prd-zen-user-widgets-v2 UW38): the manifest,
   *  bundle and grant as the daemon resolved them. `caps` above is then
   *  the effective caps and `source` is always `user` (UW26). */
  custom?: ZenCustomWidgetPayload
}

/** The kind of a custom (agent-written) widget placement (UW6). */
export const ZEN_CUSTOM_KIND = 'custom'

/** One theme the daemon offers (Omarchy additions 1–3): K2's built-in
 *  read-only themes and the user's own under `~/.k2/zen/themes/`. */
export interface ZenThemeEntry {
  /** The id (lower case): what a switch sends. */
  name: string
  /** What the picker shows: "Basic", "Paper", "Midnight" (Rosson 2026-10-04). */
  label: string
  /** Shipped inside K2 (read-only). */
  builtin: boolean
  /** `~/.k2/zen/themes/<name>/theme.toml` exists (for a built-in: an override). */
  user: boolean
}

/** Where the active theme was picked: for every Garden, or this Garden
 *  only (G16). */
export type ZenThemeScope = 'global' | 'garden'

/** Which Garden the page is (G12). */
export interface ZenPageGarden {
  id: string
  name: string
  /** 1-based position in the Garden list. */
  index: number
}

export interface ZenResolvedPage {
  schema: 1
  /** Changes whenever the resolved result changes. */
  version: string
  template: string
  layout: ZenLayout
  /** CONTENT widgets only (FC28): chrome is in `placement`. */
  widgets: ZenWidgetDecl[]
  /** Controls the page declares it draws (Z27 "declared"). */
  controls: string[]
  /** Where K2's controls sit (FC29): bands, column edges and menus. */
  placement: ZenPlacement
  /** Raw theme tables; the S5 theme engine reads them. */
  theme: unknown
  /** The active theme's name (`theme.name`), null when the daemon sends none. */
  activeTheme: string | null
  /** `theme.scope`: a switch keeps it (a Garden's own pick stays the Garden's). */
  themeScope: ZenThemeScope
  /** The Garden the daemon resolved (null when it sends none). */
  garden: ZenPageGarden | null
  /** Themes to pick from, in the daemon's order (empty when it sends none). */
  themes: ZenThemeEntry[]
  /** The window's `[chrome]` table (corners, stoplights). Not the page's
   *  controls: those are `placement`. */
  chrome: unknown
  motion: unknown
  errors: ZenIssue[]
  warnings: ZenIssue[]
  lastGoodAt: string | null
}

/** The first Garden's template, "Garden 1" (G11). */
export const BUILTIN_TEMPLATE_ID = 'k2.texting@1'
export const TEXTING_TEMPLATE_ID = BUILTIN_TEMPLATE_ID
/** A new Garden's template: one column holding the empty-Garden widget (G11). */
export const BLANK_TEMPLATE_ID = 'k2.blank@1'

/** Both templates' chrome (FC30, the daemon's template TOMLs): one top band,
 *  the Garden switcher at the start; usage, theme and the Zen toggle at the
 *  end, so the toggle sits in the top-right corner. */
export const TEMPLATE_CHROME_ITEMS: readonly ZenChromeItem[] = Object.freeze([
  { id: 'garden-switcher', kind: 'garden-switcher', slot: 'top', align: 'start', props: {}, caps: ['gardens:manage'] },
  { id: 'usage', kind: 'usage', slot: 'top', align: 'end', props: {}, caps: [] },
  { id: 'theme-picker', kind: 'theme-picker', slot: 'top', align: 'end', props: {}, caps: [] },
  { id: 'zen-toggle', kind: 'zen-toggle', slot: 'top', align: 'end', props: {}, caps: [] },
]) as readonly ZenChromeItem[]

function cloneItem(i: ZenChromeItem): ZenChromeItem {
  return { ...i, props: { ...i.props }, caps: [...i.caps] }
}

/** The template's own placement (with its content widgets' band and edge
 *  rows), as the daemon computes it. */
export function templatePlacement(widgets: readonly ZenWidgetDecl[]): ZenPlacement {
  const items = TEMPLATE_CHROME_ITEMS.map(cloneItem)
  return { from: 'template', items, ...placeRows(items, widgets, true) }
}

/** `k2.texting@1` as the renderer knows it: the safe-mode page, and the
 *  fallback for any part of a resolved page the daemon left out. Mirrors
 *  the daemon's template (`crates/k2-core/src/zen/template-k2-texting-1.toml`). */
export const BUILTIN_TEXTING_PAGE: ZenResolvedPage = Object.freeze({
  schema: 1,
  version: 'builtin',
  template: BUILTIN_TEMPLATE_ID,
  layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] },
  widgets: [
    {
      id: 'agents',
      kind: 'agents',
      column: 0,
      props: { 'home-picker': true },
      caps: ['agents:read', 'agents:add', 'presence:read'],
      source: 'builtin',
    },
    {
      id: 'conversation',
      kind: 'conversation',
      column: 1,
      props: { agents: 'agents' },
      caps: ['agents:read', 'presence:read', 'thread:read', 'thread:post'],
      source: 'builtin',
    },
    // The thin left rail (Rosson 2026-10-04): left edge of column 0.
    { id: 'nav', kind: 'nav-rail', column: 0, props: {}, caps: ['app:navigate'], source: 'builtin' },
  ],
  controls: ['garden-switcher', 'drag-region', 'zen-toggle', 'add-agent'],
  placement: templatePlacement([]),
  theme: null,
  activeTheme: null,
  themeScope: 'global',
  garden: null,
  themes: [],
  chrome: null,
  motion: null,
  errors: [],
  warnings: [],
  lastGoodAt: null,
}) as ZenResolvedPage

/** `k2.blank@1`: one column holding the empty-Garden widget, and the
 *  same chrome (no Add agent). Mirrors the daemon's `template-k2-blank-1.toml`. */
export const BUILTIN_BLANK_PAGE: ZenResolvedPage = Object.freeze({
  ...BUILTIN_TEXTING_PAGE,
  template: BLANK_TEMPLATE_ID,
  layout: { kind: 'columns', split: [100], minWidths: [320] },
  widgets: [
    {
      id: 'garden-empty',
      kind: 'garden-empty',
      column: 0,
      props: {},
      caps: ['agents:read', 'thread:read', 'thread:post', 'gardens:template'],
      source: 'builtin',
    },
  ],
  controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
  placement: templatePlacement([]),
}) as ZenResolvedPage

/** The built-in page of a template id (texting for any other id). */
export function builtinPageFor(template: string): ZenResolvedPage {
  return template === BLANK_TEMPLATE_ID ? BUILTIN_BLANK_PAGE : BUILTIN_TEXTING_PAGE
}

/** The daemon sent something this client can't read as a Zen page. */
export class ZenPageParseError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ZenPageParseError'
  }
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function num(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

function align(v: unknown): ZenAlign | undefined {
  return typeof v === 'string' && (ZEN_ALIGNS as readonly string[]).includes(v) ? (v as ZenAlign) : undefined
}

function edgeOf(v: unknown): ZenEdge | undefined {
  return typeof v === 'string' && (ZEN_EDGES as readonly string[]).includes(v) ? (v as ZenEdge) : undefined
}

function parseIssues(raw: unknown): ZenIssue[] {
  if (!Array.isArray(raw)) return []
  const out: ZenIssue[] = []
  for (const i of raw) {
    if (!isObj(i) || typeof i.message !== 'string') continue
    out.push({
      file: typeof i.file === 'string' ? i.file : '',
      line: num(i.line) ?? 0,
      col: num(i.col) ?? 0,
      message: i.message,
    })
  }
  return out
}

/** G38: `[[layout.column]]` tables (`size` percent, `min-width` px), when
 *  the daemon sends the column list instead of `split` / `minWidths`. */
function columnsLayout(raw: unknown[]): ZenLayout | null {
  const split: number[] = []
  const mins: number[] = []
  for (const c of raw) {
    if (!isObj(c)) return null
    const size = num(c.size)
    if (size === null || size <= 0) return null
    split.push(size)
    mins.push(Math.max(0, num(c.minWidth ?? c['min-width'] ?? c.min_width) ?? 0))
  }
  return split.length > 0 ? { kind: 'columns', split, minWidths: mins } : null
}

function parseLayout(raw: unknown, builtin: ZenResolvedPage): ZenLayout {
  const fallback = builtin.layout
  if (!isObj(raw)) return { ...fallback, split: [...fallback.split], minWidths: [...fallback.minWidths] }
  if (!Array.isArray(raw.split) && Array.isArray(raw.columns)) {
    const cols = columnsLayout(raw.columns)
    if (cols) return cols
  }
  const split = Array.isArray(raw.split) ? raw.split.map(num).filter((n): n is number => n !== null && n > 0) : []
  const cols = split.length > 0 ? split : [...fallback.split]
  const rawMins = raw.minWidths ?? raw.min_widths
  const mins = Array.isArray(rawMins)
    ? rawMins.map((n) => Math.max(0, num(n) ?? 0))
    : split.length > 0
      ? cols.map(() => 0)
      : [...fallback.minWidths]
  while (mins.length < cols.length) mins.push(0)
  return { kind: 'columns', split: cols, minWidths: mins.slice(0, cols.length) }
}

function parseWidgets(raw: unknown, columns: number, builtin: ZenResolvedPage): ZenWidgetDecl[] {
  if (!Array.isArray(raw)) return builtin.widgets.map((w) => ({ ...w, props: { ...w.props }, caps: [...w.caps] }))
  const out: ZenWidgetDecl[] = []
  raw.forEach((w, idx) => {
    if (!isObj(w)) return
    const kind = typeof w.kind === 'string' ? w.kind : typeof w.type === 'string' ? w.type : null
    if (!kind) return
    // FC28: chrome never comes in `widgets`; a daemon that sent one anyway
    // would get a placeholder box in a column, so it is left out here.
    if ((ZEN_CHROME_KINDS as readonly string[]).includes(kind)) return
    const id = typeof w.id === 'string' && w.id ? w.id : `${kind}-${idx}`
    const col = num(w.column ?? w.col)
    const band = typeof w.slot === 'string' && ZEN_BAND_SLOTS.includes(w.slot) ? w.slot : null
    const e = band ? undefined : edgeOf(w.edge)
    const a = align(w.align)
    const column = band ? 0 : col !== null && col >= 0 && col < columns ? Math.floor(col) : Math.min(idx, columns - 1)
    const props = isObj(w.props) ? w.props : {}
    if (kind === ZEN_CUSTOM_KIND) {
      // UW6, UWA14: column-only content; the renderer forces `source: user`
      // and the effective caps whatever the body says (UW26).
      const custom = parseZenCustomWidget(w, { id, column, props })
      out.push({ id, kind, column, props, caps: [...custom.caps], source: 'user', custom })
      return
    }
    out.push({
      id,
      kind,
      ...(band ? { slot: band } : {}),
      ...(e ? { edge: e } : {}),
      ...(a ? { align: a } : {}),
      column,
      props,
      caps: Array.isArray(w.caps) ? w.caps.filter((c): c is string => typeof c === 'string') : [],
      source: typeof w.source === 'string' ? w.source : 'builtin',
    })
  })
  return out
}

function parseControls(raw: unknown): string[] {
  if (!Array.isArray(raw)) return []
  const out: string[] = []
  for (const c of raw) {
    if (typeof c === 'string') out.push(c)
    else if (isObj(c) && typeof c.kind === 'string') out.push(c.kind)
  }
  return out
}

// ── Placement (prd-zen-freeform-chrome FC29) ─────────────────────────────

type Spot = { kind: 'band'; band: string } | { kind: 'edge'; column: number; edge: ZenEdge } | { kind: 'menu'; menu: string } | { kind: 'body' }

/** Where a content widget is drawn: a band, a column edge (a row rail, or
 *  one with an `edge`), or the body of its column. Mirrors the daemon's
 *  `spot_of`. */
function contentSpot(w: ZenWidgetDecl): Spot {
  if (zenWidgetInBand(w)) return { kind: 'band', band: zenWidgetSlot(w) }
  const row = w.edge !== undefined || (w.kind === 'nav-rail' && w.props.orientation === 'row')
  return row ? { kind: 'edge', column: w.column, edge: w.edge ?? 'top' } : { kind: 'body' }
}

function chromeSpot(i: ZenChromeItem): Spot {
  if (ZEN_BAND_SLOTS.includes(i.slot)) return { kind: 'band', band: i.slot }
  if (i.slot === ZEN_MENU_SLOT) return { kind: 'menu', menu: i.menu ?? '' }
  return { kind: 'edge', column: i.column ?? 0, edge: i.edge ?? 'top' }
}

function emptyGroups(): ZenRowGroups {
  return { start: [], center: [], end: [] }
}

/**
 * `bands`, `edges` and `menus` from the items alone, the way the daemon's
 * `place_rows` does: file order inside each group, and the TEMPLATE's
 * chrome keeps its corners (its `start` items lead their group, its other
 * items close theirs), so a band widget sits between the Garden switcher
 * and the toggle. Used when the daemon sends no rows (FC32).
 */
function placeRows(
  items: readonly ZenChromeItem[],
  widgets: readonly ZenWidgetDecl[],
  templateChrome: boolean,
): Pick<ZenPlacement, 'bands' | 'edges' | 'menus'> {
  type Entry = { id: string; spot: Spot; align: ZenAlign; rank: number }
  const entries: Entry[] = []
  for (const w of widgets) entries.push({ id: w.id, spot: contentSpot(w), align: w.align ?? 'start', rank: 1 })
  for (const i of items) {
    const a = i.align ?? 'start'
    entries.push({ id: i.id, spot: chromeSpot(i), align: a, rank: templateChrome ? (a === 'start' ? 0 : 2) : 1 })
  }
  // A stable sort keeps file order inside each rank.
  const ordered = entries.map((e, n) => ({ e, n })).sort((x, y) => x.e.rank - y.e.rank || x.n - y.n).map((x) => x.e)
  const bands: Record<string, ZenRowGroups> = {}
  const edges = new Map<string, ZenColumnEdge>()
  const menus: Record<string, string[]> = {}
  for (const i of items) if (i.kind === 'menu') menus[i.id] = []
  for (const e of ordered) {
    const s = e.spot
    if (s.kind === 'body') continue
    if (s.kind === 'menu') {
      ;(menus[s.menu] ??= []).push(e.id)
      continue
    }
    const g =
      s.kind === 'band'
        ? (bands[s.band] ??= emptyGroups())
        : (() => {
            const key = `${s.column}:${s.edge}`
            let edge = edges.get(key)
            if (!edge) {
              edge = { column: s.column, edge: s.edge, ...emptyGroups() }
              edges.set(key, edge)
            }
            return edge
          })()
    g[e.align].push(e.id)
  }
  const sortedEdges = [...edges.values()].sort((a, b) => a.column - b.column || (a.edge === b.edge ? 0 : a.edge === 'top' ? -1 : 1))
  return { bands: { top: bands.top ?? null, bottom: bands.bottom ?? null }, edges: sortedEdges, menus }
}

function parseChromeItem(raw: unknown): ZenChromeItem | null {
  if (!isObj(raw) || typeof raw.kind !== 'string' || !raw.kind) return null
  const slot = typeof raw.slot === 'string' && raw.slot ? raw.slot : ZEN_COLUMN_SLOT
  const col = num(raw.column)
  const e = edgeOf(raw.edge)
  const a = align(raw.align)
  return {
    id: typeof raw.id === 'string' && raw.id ? raw.id : raw.kind,
    kind: raw.kind,
    slot,
    ...(slot === ZEN_COLUMN_SLOT ? { column: col !== null && col >= 0 ? Math.floor(col) : 0 } : {}),
    ...(e ? { edge: e } : {}),
    ...(a ? { align: a } : {}),
    ...(typeof raw.menu === 'string' ? { menu: raw.menu } : {}),
    props: isObj(raw.props) ? raw.props : {},
    caps: Array.isArray(raw.caps) ? raw.caps.filter((c): c is string => typeof c === 'string') : [],
  }
}

function idList(raw: unknown, known: ReadonlySet<string>): string[] {
  if (!Array.isArray(raw)) return []
  return raw.filter((id): id is string => typeof id === 'string' && known.has(id))
}

function parseGroups(raw: unknown, known: ReadonlySet<string>): ZenRowGroups | null {
  if (!isObj(raw)) return null
  return { start: idList(raw.start, known), center: idList(raw.center, known), end: idList(raw.end, known) }
}

function parsePlacement(page: Record<string, unknown>, widgets: ZenWidgetDecl[], columns: number): ZenPlacement {
  const chrome = page.chrome
  // FC32: a daemon without `zen-chrome-v1` (no `page.chrome`) → the template's.
  if (!isObj(chrome) || !Array.isArray(chrome.items)) return templatePlacement(widgets)
  const items: ZenChromeItem[] = []
  const seen = new Set<string>()
  for (const raw of chrome.items) {
    const item = parseChromeItem(raw)
    if (!item || seen.has(item.id)) continue
    seen.add(item.id)
    // An edge item in a column the layout lacks sits in the last column.
    if (item.slot === ZEN_COLUMN_SLOT && (item.column ?? 0) >= columns) item.column = columns - 1
    items.push(item)
  }
  const from = chrome.from === 'garden' ? 'garden' : 'template'
  const computed = placeRows(items, widgets, from === 'template')
  const known = new Set<string>([...items.map((i) => i.id), ...widgets.map((w) => w.id)])
  const bandsRaw = page.bands
  const bands = isObj(bandsRaw)
    ? { top: parseGroups(bandsRaw.top, known), bottom: parseGroups(bandsRaw.bottom, known) }
    : computed.bands
  const edges = Array.isArray(page.edges)
    ? page.edges.flatMap((e): ZenColumnEdge[] => {
        if (!isObj(e)) return []
        const column = num(e.column)
        const edge = edgeOf(e.edge)
        if (column === null || column < 0 || !edge) return []
        const g = parseGroups(e, known) as ZenRowGroups
        return [{ column: Math.min(Math.floor(column), columns - 1), edge, ...g }]
      })
    : computed.edges
  const menus: Record<string, string[]> = {}
  if (isObj(page.menus)) {
    for (const [id, list] of Object.entries(page.menus)) menus[id] = idList(list, known)
  } else {
    Object.assign(menus, computed.menus)
  }
  return { from, items, bands, edges, menus }
}

/** The chrome item with `id` on the page, or null. */
export function zenChromeItem(placement: ZenPlacement, id: string): ZenChromeItem | null {
  return placement.items.find((i) => i.id === id) ?? null
}

/** Where required control `kind` sits: the item, and the menu holding it
 *  (when it is inside one). Null when the page doesn't place it. */
export function zenControlPlacement(
  placement: ZenPlacement,
  kind: ZenControlKind,
): { item: ZenChromeItem; menu: ZenChromeItem | null } | null {
  const item = placement.items.find((i) => i.kind === kind)
  if (!item) return null
  if (item.slot !== ZEN_MENU_SLOT) return { item, menu: null }
  const menu = placement.items.find((i) => i.kind === 'menu' && i.id === item.menu) ?? null
  return { item, menu }
}

/** The required controls menu `id` holds, in its order (FC25 wiring). */
export function zenMenuRequiredControls(placement: ZenPlacement, id: string): ZenControlKind[] {
  const out: ZenControlKind[] = []
  for (const itemId of placement.menus[id] ?? []) {
    const kind = zenChromeItem(placement, itemId)?.kind
    if (kind && (ZEN_REQUIRED_CONTROLS as readonly string[]).includes(kind)) out.push(kind as ZenControlKind)
  }
  return out
}

/** A menu button's name (FC15): its `label`, else "More". */
export function zenMenuLabel(item: Pick<ZenChromeItem, 'props'>): string {
  const label = item.props.label
  return typeof label === 'string' && label.trim() ? label.trim() : 'More'
}

/** A theme's display name when the daemon sends none: the id with its
 *  first letter capitalized (`basic` → `Basic`). */
export function zenThemeLabel(name: string): string {
  return name.charAt(0).toUpperCase() + name.slice(1)
}

function parseThemes(raw: unknown): ZenThemeEntry[] {
  if (!Array.isArray(raw)) return []
  const out: ZenThemeEntry[] = []
  const seen = new Set<string>()
  for (const t of raw) {
    const name = typeof t === 'string' ? t : isObj(t) && typeof t.name === 'string' ? t.name : null
    if (!name || seen.has(name)) continue
    seen.add(name)
    const label = isObj(t) && typeof t.label === 'string' && t.label.trim() ? t.label : zenThemeLabel(name)
    out.push({ name, label, builtin: isObj(t) && t.builtin === true, user: isObj(t) && t.user === true })
  }
  return out
}

function parseGarden(raw: unknown): ZenPageGarden | null {
  if (!isObj(raw) || typeof raw.id !== 'string' || !raw.id) return null
  return {
    id: raw.id,
    name: typeof raw.name === 'string' ? raw.name : raw.id,
    index: num(raw.index) ?? 0,
  }
}

/**
 * Parse a `GET /cli/zen/get` body. Throws `ZenPageParseError` when there is
 * no page to draw (not an object, `ok: false` without a page, no `page`).
 * Missing optional parts fall back to the built-in template; `controls` does
 * NOT fall back (a page that declares none fails the "declared" check).
 */
export function parseZenGet(raw: unknown): ZenResolvedPage {
  if (!isObj(raw)) throw new ZenPageParseError('the Zen page is not an object')
  const page = raw.page
  if (!isObj(page)) {
    const why = typeof raw.error === 'string' ? raw.error : 'no page in the answer'
    throw new ZenPageParseError(why)
  }
  if (raw.schema !== undefined && raw.schema !== 1) {
    throw new ZenPageParseError(`this K2 reads Zen schema 1, the page is schema ${String(raw.schema)}`)
  }
  const template = typeof page.template === 'string' ? page.template : BUILTIN_TEMPLATE_ID
  const builtin = builtinPageFor(template)
  const layout = parseLayout(page.layout, builtin)
  const widgets = parseWidgets(page.widgets, layout.split.length, builtin)
  const themes = parseThemes(raw.themes)
  const named = isObj(raw.theme) && typeof raw.theme.name === 'string' && raw.theme.name ? raw.theme.name : null
  const flagged = Array.isArray(raw.themes)
    ? raw.themes.find((t): t is { name: string } => isObj(t) && t.active === true && typeof t.name === 'string')
    : undefined
  return {
    schema: 1,
    version: typeof raw.version === 'string' ? raw.version : String(raw.version ?? ''),
    template,
    layout,
    widgets,
    controls: parseControls(page.controls),
    placement: parsePlacement(page, widgets, layout.split.length),
    theme: raw.theme ?? null,
    activeTheme: named ?? flagged?.name ?? null,
    themeScope: isObj(raw.theme) && raw.theme.scope === 'garden' ? 'garden' : 'global',
    garden: parseGarden(raw.garden),
    themes,
    chrome: raw.chrome ?? null,
    motion: raw.motion ?? null,
    errors: parseIssues(raw.errors),
    warnings: parseIssues(raw.warnings),
    lastGoodAt: typeof raw.lastGoodAt === 'string' ? raw.lastGoodAt : null,
  }
}

/** Z13 banner line for the first error: "zen.toml line 12: unknown color
 *  'acent'. Showing your last good version." */
export function zenErrorBannerText(issue: ZenIssue): string {
  const where = issue.file ? `${issue.file} line ${issue.line}` : `line ${issue.line}`
  const msg = issue.message.replace(/[.\s]+$/, '')
  return `${where}: ${msg}. Showing your last good version.`
}
