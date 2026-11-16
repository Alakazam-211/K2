// A PDF's layers (optional content groups), as the viewer's Layers panel
// shows them. The source of truth is pdf.js's OptionalContentConfig, one per
// tab: it starts from the PDF's default config, a switch sets one group, and
// the page re-renders with it (optionalContentConfigPromise). Nothing is
// written to the file. The panel lists groups in the PDF's /Order (nested
// label groups flatten, with a depth), then any group the order leaves out.

/** The part of pdf.js's OptionalContentConfig the viewer uses. */
export interface OcConfig {
  getOrder(): unknown[] | null
  getGroup(id: string): { name: string | null; visible: boolean } | null
  setVisibility(id: string, visible?: boolean, preserveRB?: boolean): void
  [Symbol.iterator](): Iterator<[string, { name: string | null; visible: boolean }]>
}

export interface PdfLayer {
  id: string
  name: string
  visible: boolean
  /** Nesting under a label group in the PDF's order (0: top level). */
  depth: number
}

export function readLayers(cfg: OcConfig | null): PdfLayer[] {
  if (!cfg) return []
  const out: PdfLayer[] = []
  const seen = new Set<string>()
  const add = (id: string, depth: number): void => {
    const g = cfg.getGroup(id)
    if (!g || seen.has(id)) return
    seen.add(id)
    out.push({ id, name: g.name?.trim() || 'Unnamed layer', visible: g.visible, depth })
  }
  const walk = (items: unknown[], depth: number): void => {
    for (const it of items) {
      if (typeof it === 'string') add(it, depth)
      else if (Array.isArray(it)) walk(it, depth + 1)
      else if (it && typeof it === 'object' && Array.isArray((it as { order?: unknown }).order)) walk((it as { order: unknown[] }).order, depth + 1)
    }
  }
  walk(cfg.getOrder() ?? [], 0)
  for (const [id] of cfg) add(id, 0)
  return out
}

/** One layer on or off. A radio-button group (/RBGroups) turns its others off, as the PDF asks. */
export function setLayer(cfg: OcConfig, id: string, visible: boolean): void {
  cfg.setVisibility(id, visible)
}

export function setAllLayers(cfg: OcConfig, visible: boolean): void {
  for (const [id] of cfg) cfg.setVisibility(id, visible, false)
}

export const visibleCount = (layers: readonly PdfLayer[]): number => layers.filter((l) => l.visible).length

/** The status line's words: "3 of 4 layers". */
export function layerSummary(layers: readonly PdfLayer[]): string {
  if (layers.length === 0) return 'No layers'
  return `${visibleCount(layers)} of ${layers.length} ${layers.length === 1 ? 'layer' : 'layers'}`
}

/** A new version of the file: keep this tab's choices, by layer name (a re-export renumbers objects). */
export function keepChoices(cfg: OcConfig, choices: ReadonlyMap<string, boolean>): void {
  if (choices.size === 0) return
  for (const l of readLayers(cfg)) {
    const want = choices.get(l.name)
    if (want !== undefined && want !== l.visible) cfg.setVisibility(l.id, want, false)
  }
}
