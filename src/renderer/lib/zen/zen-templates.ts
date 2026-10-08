// prd-zen-user-widgets-v2 UWB23 and R5 as changed (2026-10-08) — the Garden
// starts and the Garden catalog, from THIS computer's daemon.
//
//   GET /cli/zen/templates → {ok, templates: [TemplateInfo]}
//
// One list feeds every place that offers a template: New Garden in Zen
// (`ZenNewGarden`), Settings → Gardens, the bridge's `gardens.create` /
// `gardens.useTemplate` check and the template names Settings shows. Two
// sections: `start` (Start with the default, Start empty and ask my agent)
// and `catalog` (ready-made Gardens; the Diary is the first). Adding a
// catalog Garden is a data file in k2-core, so it shows here with no
// renderer change. A catalog Garden is only ever created from New Garden;
// nothing here appends one to a person's Gardens.
//
// An older daemon (no templates route) or a failed read falls back to the
// two starts (`ZEN_TEMPLATES_FALLBACK`): no catalog, nothing breaks.

import { create } from 'zustand'
import { daemonCliGet } from '@/lib/daemon-cli'
import { zenLocalScope } from './zen-api'
import { zenWidgetCaps } from './zen-custom-payload'
import { ZEN_TEMPLATES_FALLBACK, type ZenTemplateInfo } from './zen-custom-types'

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** Parse `GET /cli/zen/templates`. Rows it can't read are dropped; a body
 *  with no list throws. */
export function parseZenTemplates(raw: unknown): ZenTemplateInfo[] {
  if (!isObj(raw) || !Array.isArray(raw.templates)) throw new Error('zen/templates: no templates in the answer')
  const out: ZenTemplateInfo[] = []
  const seen = new Set<string>()
  for (const t of raw.templates) {
    if (!isObj(t) || typeof t.id !== 'string' || typeof t.short !== 'string' || !t.short || seen.has(t.short)) continue
    if (t.section !== 'start' && t.section !== 'catalog') continue
    seen.add(t.short)
    const g = t.needsGrant
    out.push({
      id: t.id,
      short: t.short,
      label: typeof t.label === 'string' && t.label.trim() ? t.label : t.short,
      description: typeof t.description === 'string' ? t.description : '',
      section: t.section,
      needsGrant: isObj(g) && typeof g.widget === 'string' ? { widget: g.widget, caps: zenWidgetCaps(g.caps) } : null,
      newUsers: t.newUsers === true,
    })
  }
  return out
}

export interface ZenTemplatesState {
  status: 'idle' | 'loading' | 'ready' | 'fallback'
  templates: readonly ZenTemplateInfo[]
}

export const useZenTemplatesStore = create<ZenTemplatesState>(() => ({
  status: 'idle',
  templates: ZEN_TEMPLATES_FALLBACK,
}))

let loadSeq = 0

/** Read the list (again). Never throws: a failure keeps the two starts. */
export async function loadZenTemplates(): Promise<readonly ZenTemplateInfo[]> {
  const seq = ++loadSeq
  useZenTemplatesStore.setState({ status: 'loading' })
  let next: ZenTemplatesState
  try {
    const list = parseZenTemplates(await daemonCliGet<unknown>(zenLocalScope(), 'zen/templates'))
    next = { status: 'ready', templates: list.length > 0 ? list : ZEN_TEMPLATES_FALLBACK }
  } catch (err) {
    console.warn('[zen] Garden templates unavailable; offering the two starts:', err)
    next = { status: 'fallback', templates: ZEN_TEMPLATES_FALLBACK }
  }
  if (seq === loadSeq) useZenTemplatesStore.setState(next)
  return next.templates
}

/** The starts, then the catalog, in the daemon's order. */
export function zenTemplateSections(list: readonly ZenTemplateInfo[]): {
  starts: ZenTemplateInfo[]
  catalog: ZenTemplateInfo[]
} {
  return { starts: list.filter((t) => t.section === 'start'), catalog: list.filter((t) => t.section === 'catalog') }
}

/** A template by its short name (`texting`, `diary`), from the list K2 has now. */
export function zenTemplateByShort(short: string): ZenTemplateInfo | null {
  return useZenTemplatesStore.getState().templates.find((t) => t.short === short) ?? null
}

/** Is `short` a template this computer offers? (`texting` and `blank` always.) */
export function isZenTemplateShort(short: unknown): short is string {
  if (typeof short !== 'string' || !short) return false
  return short === 'texting' || short === 'blank' || zenTemplateByShort(short) !== null
}

/** What a template id is called (Settings shows it per Garden). */
export function zenTemplateName(id: string): string {
  if (id === 'k2.texting@1') return 'Default layout'
  if (id === 'k2.blank@1') return 'Empty'
  const t = useZenTemplatesStore.getState().templates.find((x) => x.id === id)
  if (t) return t.label
  // `k2.diary@1` before the list is read: "Diary".
  const m = /^k2\.([a-z0-9_-]+)@\d+$/.exec(id)
  if (m) return m[1].charAt(0).toUpperCase() + m[1].slice(1)
  return id || 'Unknown'
}

// ── Catalog badges ────────────────────────────────────────────────────────

/** A short word on a catalog entry ("New"), keyed by the template's short
 *  name. Whoever knows what's new (the Garden sync builder's What's new
 *  card) sets it; the catalog only draws it. Per-window view state. */
export const useZenCatalogBadges = create<{ badges: Record<string, string> }>(() => ({ badges: {} }))

export function setZenCatalogBadge(short: string, text: string | null): void {
  useZenCatalogBadges.setState((s) => {
    const badges = { ...s.badges }
    if (text) badges[short] = text
    else delete badges[short]
    return { badges }
  })
}

/** A free name for a Garden added from the catalog: its label, else
 *  "<label> 2", "<label> 3", … (case aside, like the daemon). */
export function zenCatalogGardenName(label: string, taken: readonly string[]): string {
  const used = new Set(taken.map((n) => n.toLocaleLowerCase()))
  const base = label.trim().slice(0, 56) || 'Garden'
  if (!used.has(base.toLocaleLowerCase())) return base
  for (let n = 2; ; n++) {
    const name = `${base} ${n}`
    if (!used.has(name.toLocaleLowerCase())) return name
  }
}

/** Tests only. */
export function __resetZenTemplatesForTests(): void {
  loadSeq = 0
  useZenTemplatesStore.setState({ status: 'idle', templates: ZEN_TEMPLATES_FALLBACK })
  useZenCatalogBadges.setState({ badges: {} })
}
