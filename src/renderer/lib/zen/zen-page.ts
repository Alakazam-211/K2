// prd-zen-mode-v1 Z9, Z10, Z15 — the resolved Zen page, as the renderer
// reads it from `GET /cli/zen/get?home=<id>` on THIS computer's daemon.
//
// The daemon resolves template + `zen.toml` + the Home's page override and
// returns the structure; the renderer draws from that structure even in v1
// (only the theme is the user's in v1). Every body is parsed here, at the
// boundary: nothing past `parseZenGet` sees raw JSON. The inner shapes of
// `layout`, `widgets` and `controls` are this client's reading of the
// contract; see `docs/zen-contract.md`.
//
// The built-in `k2.texting@1` page is also the safe-mode page (Z29): safe
// mode never reads the user's files, so it renders `BUILTIN_TEXTING_PAGE`.

/** The three controls every Zen page must draw (Z27). */
export const ZEN_REQUIRED_CONTROLS = ['zen-toggle', 'home-switcher', 'drag-region'] as const
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

/** One widget placed on the page. Built-ins in v1; the same shape carries
 *  v2 user widgets. `caps` are what its source declared (Z33). */
export interface ZenWidgetDecl {
  id: string
  /** Which widget renders it: `agents`, `conversation` (v1 built-ins). */
  kind: string
  /** Column index in `layout`. */
  column: number
  props: Record<string, unknown>
  caps: string[]
  /** `builtin` in v1 (granted by K2). */
  source: string
}

/** One theme the daemon offers (Omarchy additions 1–3): K2's built-in
 *  read-only themes and the user's own under `~/.k2/zen/themes/`. */
export interface ZenThemeEntry {
  name: string
  builtin: boolean
}

export interface ZenResolvedPage {
  schema: 1
  /** Changes whenever the resolved result changes. */
  version: string
  template: string
  layout: ZenLayout
  widgets: ZenWidgetDecl[]
  /** Controls the page declares it draws (Z27 "declared"). */
  controls: string[]
  /** Raw theme tables; the S5 theme engine reads them. */
  theme: unknown
  /** The active theme's name (`theme.name`), null when the daemon sends none. */
  activeTheme: string | null
  /** Themes to pick from, in the daemon's order (empty when it sends none). */
  themes: ZenThemeEntry[]
  chrome: unknown
  motion: unknown
  errors: ZenIssue[]
  warnings: ZenIssue[]
  lastGoodAt: string | null
}

export const BUILTIN_TEMPLATE_ID = 'k2.texting@1'

/** `k2.texting@1` as the renderer knows it: the safe-mode page, and the
 *  fallback for any part of a resolved page the daemon left out. */
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
      props: {},
      caps: ['agents:read', 'presence:read'],
      source: 'builtin',
    },
    {
      id: 'conversation',
      kind: 'conversation',
      column: 1,
      props: {},
      caps: ['agents:read', 'presence:read', 'thread:read', 'thread:post'],
      source: 'builtin',
    },
  ],
  controls: [...ZEN_REQUIRED_CONTROLS],
  theme: null,
  activeTheme: null,
  themes: [],
  chrome: null,
  motion: null,
  errors: [],
  warnings: [],
  lastGoodAt: null,
}) as ZenResolvedPage

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

function parseLayout(raw: unknown): ZenLayout {
  const fallback = BUILTIN_TEXTING_PAGE.layout
  if (!isObj(raw)) return { ...fallback, split: [...fallback.split], minWidths: [...fallback.minWidths] }
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

function parseWidgets(raw: unknown, columns: number): ZenWidgetDecl[] {
  if (!Array.isArray(raw)) return BUILTIN_TEXTING_PAGE.widgets.map((w) => ({ ...w, caps: [...w.caps] }))
  const out: ZenWidgetDecl[] = []
  raw.forEach((w, idx) => {
    if (!isObj(w)) return
    const kind = typeof w.kind === 'string' ? w.kind : typeof w.type === 'string' ? w.type : null
    if (!kind) return
    const id = typeof w.id === 'string' && w.id ? w.id : `${kind}-${idx}`
    const col = num(w.column ?? w.col)
    out.push({
      id,
      kind,
      column: col !== null && col >= 0 && col < columns ? Math.floor(col) : Math.min(idx, columns - 1),
      props: isObj(w.props) ? w.props : {},
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

function parseThemes(raw: unknown): ZenThemeEntry[] {
  if (!Array.isArray(raw)) return []
  const out: ZenThemeEntry[] = []
  const seen = new Set<string>()
  for (const t of raw) {
    const name = typeof t === 'string' ? t : isObj(t) && typeof t.name === 'string' ? t.name : null
    if (!name || seen.has(name)) continue
    seen.add(name)
    out.push({ name, builtin: isObj(t) && t.builtin === true })
  }
  return out
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
  const layout = parseLayout(page.layout)
  return {
    schema: 1,
    version: typeof raw.version === 'string' ? raw.version : String(raw.version ?? ''),
    template: typeof page.template === 'string' ? page.template : BUILTIN_TEMPLATE_ID,
    layout,
    widgets: parseWidgets(page.widgets, layout.split.length),
    controls: parseControls(page.controls),
    theme: raw.theme ?? null,
    activeTheme: isObj(raw.theme) && typeof raw.theme.name === 'string' && raw.theme.name ? raw.theme.name : null,
    themes: parseThemes(raw.themes),
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
